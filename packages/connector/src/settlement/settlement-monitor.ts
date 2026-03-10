/**
 * Settlement Monitor
 *
 * Monitors peer account balances and triggers settlement events when
 * balances exceed configured thresholds. Enables proactive settlement
 * BEFORE credit limits are reached, preventing packet rejections.
 *
 * **Threshold Detection Strategy:**
 * - Periodic polling of AccountManager balance queries (default: 30s interval)
 * - Monitors creditBalance (how much peer owes us)
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
 * - AccountManager (Story 6.3): Balance queries
 * - SettlementAPI (Story 6.7): Listens for SETTLEMENT_REQUIRED events
 * - TelemetryEmitter (Optional): Dashboard visualization
 *
 * @packageDocumentation
 */

import { EventEmitter } from 'events';
import type { Logger } from 'pino';
import type { AccountManager } from './account-manager';
import type { SettlementThresholdConfig, SettlementTriggerEvent } from '../config/types';
import { SettlementState } from '../config/types';
import type { TelemetryEmitter } from '../telemetry/telemetry-emitter';

/**
 * Settlement Monitor Configuration
 *
 * Defines peers and tokens to monitor for settlement threshold detection.
 *
 * @property thresholds - Settlement threshold configuration
 * @property peers - List of peer IDs to monitor
 * @property tokenIds - List of token IDs to monitor (default: ['M2M'] for MVP)
 * @property telemetryEmitter - Optional telemetry emitter for dashboard visualization
 * @property nodeId - Connector node ID for telemetry event identification
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

  /**
   * Optional telemetry emitter for dashboard visualization
   * When provided, emits SETTLEMENT_TRIGGERED events to dashboard
   * Story 6.8 integration point - not required for Story 6.6
   */
  telemetryEmitter?: TelemetryEmitter;

  /**
   * Connector node ID (e.g., "connector-a")
   * Required for telemetry event nodeId field (Story 6.8)
   * Used to identify which connector emitted the telemetry event
   */
  nodeId?: string;
}

/**
 * Settlement Monitor
 *
 * Monitors account balances and emits settlement trigger events when
 * thresholds are exceeded. Implements state machine to prevent duplicate
 * triggers and coordinates with SettlementAPI (Story 6.7) for settlement execution.
 *
 * **Events:**
 * - 'SETTLEMENT_REQUIRED': Emitted when balance exceeds threshold (SettlementTriggerEvent payload)
 *
 * **State Transitions:**
 * - IDLE → SETTLEMENT_PENDING: Balance exceeds threshold (first crossing)
 * - SETTLEMENT_PENDING → SETTLEMENT_IN_PROGRESS: Settlement API starts execution
 * - SETTLEMENT_IN_PROGRESS → IDLE: Settlement completes and balance reduced
 * - SETTLEMENT_PENDING → IDLE: Balance drops below threshold naturally
 *
 * @example
 * ```typescript
 * const monitor = new SettlementMonitor(config, accountManager, logger);
 *
 * // Listen for settlement triggers
 * monitor.on('SETTLEMENT_REQUIRED', async (event: SettlementTriggerEvent) => {
 *   console.log(`Settlement needed for ${event.peerId}: ${event.currentBalance} > ${event.threshold}`);
 *   await settlementAPI.executeMockSettlement(event.peerId, event.tokenId);
 * });
 *
 * // Start monitoring
 * await monitor.start();
 *
 * // Stop monitoring
 * await monitor.stop();
 * ```
 */
export class SettlementMonitor extends EventEmitter {
  private readonly _config: SettlementMonitorConfig;
  private readonly _accountManager: AccountManager;
  private readonly _logger: Logger;
  private readonly _settlementStates: Map<string, SettlementState>;
  private readonly _lastSettlementTime: Map<string, number>;
  private _pollingIntervalId: NodeJS.Timeout | null;
  private _isRunning: boolean;

  /**
   * Create a new SettlementMonitor
   *
   * @param config - Settlement monitor configuration
   * @param accountManager - AccountManager instance for balance queries
   * @param logger - Pino logger instance
   */
  constructor(config: SettlementMonitorConfig, accountManager: AccountManager, logger: Logger) {
    super();

    this._config = config;
    this._accountManager = accountManager;
    this._logger = logger.child({ component: 'settlement-monitor' });
    this._settlementStates = new Map();
    this._lastSettlementTime = new Map();
    this._pollingIntervalId = null;
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
        pollingInterval: this._config.thresholds?.pollingInterval ?? 30000,
        defaultThreshold: this._config.thresholds?.defaultThreshold?.toString(),
        peerCount: this._config.peers.length,
        tokenCount: this._config.tokenIds.length,
      },
      'Settlement monitor initialized'
    );
  }

  /**
   * Start settlement threshold monitoring
   *
   * Begins periodic polling of account balances. Runs initial check
   * immediately before starting interval timer.
   *
   * @throws Error if monitor is already running
   */
  async start(): Promise<void> {
    if (this._isRunning) {
      throw new Error('Settlement monitor already running');
    }

    this._isRunning = true;
    this._logger.info('Settlement monitor started');

    // Run initial check immediately (don't wait for first interval)
    await this._checkBalances();

    // Start polling
    const pollingInterval = this._config.thresholds?.pollingInterval ?? 30000;
    this._pollingIntervalId = setInterval(() => {
      this._checkBalances().catch((error) => {
        // Log errors but don't stop interval (handled in _checkBalances)
        this._logger.error(
          { error: error.message },
          'Uncaught error in settlement threshold polling'
        );
      });
    }, pollingInterval);
  }

  /**
   * Stop settlement threshold monitoring
   *
   * Clears polling interval and stops balance checks.
   */
  async stop(): Promise<void> {
    if (this._pollingIntervalId) {
      clearInterval(this._pollingIntervalId);
      this._pollingIntervalId = null;
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
   * Check all peer-token balances against thresholds
   *
   * Polls AccountManager for balances and emits SETTLEMENT_REQUIRED
   * events when thresholds are exceeded. Uses state machine to prevent
   * duplicate triggers.
   *
   * Errors are caught and logged to keep monitor running.
   *
   * @private
   */
  private async _checkBalances(): Promise<void> {
    try {
      this._logger.debug(
        { peerCount: this._config.peers.length },
        'Checking settlement thresholds'
      );

      for (const peerId of this._config.peers) {
        for (const tokenId of this._config.tokenIds) {
          // Get threshold for this peer-token pair
          const threshold = this._getThresholdForPeer(peerId, tokenId);

          // Skip if no threshold configured
          if (!threshold) {
            continue;
          }

          // Get current balance
          const balance = await this._accountManager.getAccountBalance(peerId, tokenId);

          // Get current settlement state
          const stateKey = `${peerId}:${tokenId}`;
          const currentState = this._settlementStates.get(stateKey) ?? SettlementState.IDLE;

          // Check if creditBalance exceeds threshold
          if (balance.creditBalance > threshold) {
            if (currentState === SettlementState.IDLE) {
              // First threshold crossing - trigger settlement
              const exceedsBy = balance.creditBalance - threshold;

              const event: SettlementTriggerEvent = {
                peerId,
                tokenId,
                currentBalance: balance.creditBalance,
                threshold,
                exceedsBy,
                timestamp: new Date(),
              };

              // Emit event
              this.emit('SETTLEMENT_REQUIRED', event);

              // Emit telemetry event if telemetry emitter configured
              this._emitTelemetry(event);

              // Update state to SETTLEMENT_PENDING
              this._settlementStates.set(stateKey, SettlementState.SETTLEMENT_PENDING);

              this._logger.warn(
                {
                  peerId,
                  tokenId,
                  balance: balance.creditBalance.toString(),
                  threshold: threshold.toString(),
                  exceedsBy: exceedsBy.toString(),
                },
                'Settlement threshold exceeded'
              );
            } else {
              // State is SETTLEMENT_PENDING or SETTLEMENT_IN_PROGRESS
              // Skip to prevent duplicate triggers
              this._logger.debug(
                { peerId, tokenId, state: currentState },
                'Settlement already pending/in-progress, skipping duplicate trigger'
              );
            }
          } else if (balance.creditBalance <= threshold) {
            // Balance below threshold
            // If state is SETTLEMENT_PENDING, reset to IDLE
            // (balance reduced naturally before settlement started)
            // When time-based triggers are active, only auto-reset when balance
            // reaches zero, otherwise the time-based trigger would re-fire immediately
            const hasTimeBasedTrigger =
              !!this._config.thresholds?.timeBasedIntervalMs && balance.creditBalance > 0n;
            if (currentState === SettlementState.SETTLEMENT_PENDING && !hasTimeBasedTrigger) {
              this._settlementStates.set(stateKey, SettlementState.IDLE);
              this._logger.info(
                { peerId, tokenId },
                'Balance returned below threshold, resetting to IDLE'
              );
            }
          }

          // Time-based settlement trigger (independent of amount threshold)
          // Re-read state in case amount-based check already triggered
          const timeInterval = this._config.thresholds?.timeBasedIntervalMs;
          const stateAfterAmountCheck =
            this._settlementStates.get(stateKey) ?? SettlementState.IDLE;
          if (
            timeInterval &&
            stateAfterAmountCheck === SettlementState.IDLE &&
            balance.creditBalance > 0n
          ) {
            const lastTime = this._lastSettlementTime.get(stateKey) ?? 0;
            if (Date.now() - lastTime >= timeInterval) {
              const event: SettlementTriggerEvent = {
                peerId,
                tokenId,
                currentBalance: balance.creditBalance,
                threshold: threshold ?? 0n,
                exceedsBy: balance.creditBalance,
                timestamp: new Date(),
              };

              this.emit('SETTLEMENT_REQUIRED', event);
              this._emitTelemetry(event);
              this._settlementStates.set(stateKey, SettlementState.SETTLEMENT_PENDING);

              this._logger.info(
                {
                  peerId,
                  tokenId,
                  balance: balance.creditBalance.toString(),
                  intervalMs: timeInterval,
                },
                'Time-based settlement triggered'
              );
            }
          }
        }
      }
    } catch (error) {
      // Log error but don't re-throw (keep monitor running)
      this._logger.error(
        {
          error: error instanceof Error ? error.message : String(error),
        },
        'Settlement threshold check failed'
      );
    }
  }

  /**
   * Emit telemetry event for settlement threshold crossing
   *
   * Sends SETTLEMENT_TRIGGERED event to dashboard via telemetry emitter.
   * Non-blocking: Errors are caught and logged to prevent threshold detection failures.
   *
   * @param event - Settlement trigger event
   * @private
   */
  private _emitTelemetry(event: SettlementTriggerEvent): void {
    if (!this._config.telemetryEmitter) {
      return;
    }

    try {
      // Emit using shared telemetry event format (Story 6.8)
      this._config.telemetryEmitter.emit({
        type: 'SETTLEMENT_TRIGGERED',
        nodeId: this._config.nodeId ?? 'unknown',
        peerId: event.peerId,
        tokenId: event.tokenId,
        currentBalance: event.currentBalance.toString(),
        threshold: event.threshold.toString(),
        exceedsBy: event.exceedsBy.toString(),
        triggerReason: 'THRESHOLD_EXCEEDED',
        timestamp: event.timestamp.toISOString(),
      });

      this._logger.debug(
        { peerId: event.peerId, tokenId: event.tokenId },
        'Settlement trigger telemetry sent to dashboard'
      );
    } catch (error) {
      // Telemetry emission is non-blocking per coding standards
      this._logger.error(
        {
          error: error instanceof Error ? error.message : String(error),
          peerId: event.peerId,
          tokenId: event.tokenId,
        },
        'Failed to emit settlement telemetry'
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
   * Called by SettlementAPI (Story 6.7) when settlement execution starts.
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
   * Called by SettlementAPI (Story 6.7) when settlement completes and
   * balance is reduced below threshold. Transitions state to IDLE,
   * ready for next threshold crossing.
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
   * Returns a copy of the internal state map for telemetry/debugging.
   *
   * @returns Map of state keys to settlement states
   */
  getAllSettlementStates(): Map<string, SettlementState> {
    // Return copy to prevent external mutation
    return new Map(this._settlementStates);
  }
}
