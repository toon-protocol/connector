/**
 * Prometheus Metrics Exporter - Production metrics collection and export
 * @packageDocumentation
 * @remarks
 * Collects and exports ILP connector metrics in Prometheus format.
 * Tracks packets, settlements, accounts, channels, and errors.
 */

import { Logger } from 'pino';
import type { RequestHandler, Request, Response } from 'express';
import type * as PromClient from 'prom-client';
import {
  PrometheusMetricsConfig,
  SettlementMethod,
  ChannelStatus,
  PacketMetricsOptions,
  SettlementMetricsOptions,
  ChannelMetricsOptions,
  ErrorMetricsOptions,
  SLAMetrics,
  ClaimMetricsOptions,
} from './types';
import { requireOptional } from '../utils/optional-require';

/**
 * Default Prometheus metrics configuration
 */
const DEFAULT_CONFIG: PrometheusMetricsConfig = {
  enabled: true,
  metricsPath: '/metrics',
  includeDefaultMetrics: true,
  labels: {},
};

/**
 * Default histogram buckets for packet latency (in seconds)
 * Covers 1ms to 1s range with exponential distribution
 */
const PACKET_LATENCY_BUCKETS = [0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1];

/**
 * Default histogram buckets for settlement latency (in seconds)
 * Covers 1s to 60s range for blockchain operations
 */
const SETTLEMENT_LATENCY_BUCKETS = [1, 3, 5, 10, 30, 60];

/**
 * Default histogram buckets for claim redemption latency (in seconds)
 * Covers 1s to 30min range for claim processing
 */
const CLAIM_REDEMPTION_LATENCY_BUCKETS = [1, 5, 10, 30, 60, 300, 600, 1800];

/**
 * PrometheusExporter collects and exports ILP connector metrics
 *
 * @example
 * ```typescript
 * const exporter = await PrometheusExporter.create(logger, { enabled: true });
 * exporter.recordPacket({ type: 'prepare', status: 'success', latencyMs: 5 });
 * const metrics = await exporter.getMetrics();
 * ```
 */
export class PrometheusExporter {
  private readonly _logger: Logger;
  private readonly _config: PrometheusMetricsConfig;
  private readonly _registry: PromClient.Registry;

  // Packet metrics
  private readonly _packetsProcessedTotal: PromClient.Counter;
  private readonly _packetLatencySeconds: PromClient.Histogram;
  private readonly _packetsInFlight: PromClient.Gauge;

  // Settlement metrics
  private readonly _settlementsExecutedTotal: PromClient.Counter;
  private readonly _settlementLatencySeconds: PromClient.Histogram;
  private readonly _settlementAmountTotal: PromClient.Counter;

  // Account metrics
  private readonly _accountBalanceUnits: PromClient.Gauge;
  private readonly _accountCreditsTotal: PromClient.Counter;
  private readonly _accountDebitsTotal: PromClient.Counter;

  // Channel metrics
  private readonly _activeChannels: PromClient.Gauge;
  private readonly _channelFundedTotal: PromClient.Counter;
  private readonly _channelClosedTotal: PromClient.Counter;
  private readonly _channelDisputesTotal: PromClient.Counter;

  // Error metrics
  private readonly _errorsTotal: PromClient.Counter;
  private readonly _lastErrorTimestamp: PromClient.Gauge;

  // Claim metrics
  private readonly _claimsSentTotal: PromClient.Counter;
  private readonly _claimsReceivedTotal: PromClient.Counter;
  private readonly _claimsRedeemedTotal: PromClient.Counter;
  private readonly _claimRedemptionLatencySeconds: PromClient.Histogram;
  private readonly _claimVerificationFailuresTotal: PromClient.Counter;
  private readonly _claimLastRedemptionTimestampSeconds: PromClient.Gauge;

  // SLA tracking (internal counters for rate calculation)
  private _packetSuccessCount: number = 0;
  private _packetTotalCount: number = 0;
  private _settlementSuccessCount: number = 0;
  private _settlementTotalCount: number = 0;
  private _latencySamples: number[] = [];

  /**
   * Create a new PrometheusExporter instance asynchronously
   *
   * @param logger - Pino logger instance
   * @param config - Prometheus metrics configuration
   * @returns Promise resolving to PrometheusExporter instance
   */
  static async create(
    logger: Logger,
    config?: Partial<PrometheusMetricsConfig>
  ): Promise<PrometheusExporter> {
    const client = await requireOptional<typeof import('prom-client')>(
      'prom-client',
      'Prometheus metrics'
    );
    return new PrometheusExporter(logger, client, config);
  }

  /**
   * Create a new PrometheusExporter instance
   *
   * @param logger - Pino logger instance
   * @param client - Loaded prom-client module
   * @param config - Prometheus metrics configuration
   */
  constructor(
    logger: Logger,
    client: typeof PromClient,
    config?: Partial<PrometheusMetricsConfig>
  ) {
    this._logger = logger.child({ component: 'prometheus-exporter' });
    this._config = { ...DEFAULT_CONFIG, ...config };

    // Create a custom registry to avoid polluting the global registry
    this._registry = new client.Registry();

    // Set default labels if provided
    if (this._config.labels && Object.keys(this._config.labels).length > 0) {
      this._registry.setDefaultLabels(this._config.labels);
    }

    // Register default Node.js metrics if enabled
    if (this._config.includeDefaultMetrics) {
      client.collectDefaultMetrics({ register: this._registry });
    }

    // Initialize packet metrics
    this._packetsProcessedTotal = new client.Counter({
      name: 'ilp_packets_processed_total',
      help: 'Total number of ILP packets processed',
      labelNames: ['type', 'status'],
      registers: [this._registry],
    });

    this._packetLatencySeconds = new client.Histogram({
      name: 'ilp_packet_latency_seconds',
      help: 'ILP packet processing latency in seconds',
      labelNames: ['type'],
      buckets: PACKET_LATENCY_BUCKETS,
      registers: [this._registry],
    });

    this._packetsInFlight = new client.Gauge({
      name: 'ilp_packets_in_flight',
      help: 'Current number of ILP packets being processed',
      registers: [this._registry],
    });

    // Initialize settlement metrics
    this._settlementsExecutedTotal = new client.Counter({
      name: 'settlements_executed_total',
      help: 'Total number of settlements executed',
      labelNames: ['method', 'status'],
      registers: [this._registry],
    });

    this._settlementLatencySeconds = new client.Histogram({
      name: 'settlement_latency_seconds',
      help: 'Settlement operation latency in seconds',
      labelNames: ['method'],
      buckets: SETTLEMENT_LATENCY_BUCKETS,
      registers: [this._registry],
    });

    this._settlementAmountTotal = new client.Counter({
      name: 'settlement_amount_total',
      help: 'Total amount settled',
      labelNames: ['method', 'token'],
      registers: [this._registry],
    });

    // Initialize account metrics
    this._accountBalanceUnits = new client.Gauge({
      name: 'account_balance_units',
      help: 'Current account balance in smallest units',
      labelNames: ['peer_id', 'token_id'],
      registers: [this._registry],
    });

    this._accountCreditsTotal = new client.Counter({
      name: 'account_credits_total',
      help: 'Total credits applied to accounts',
      labelNames: ['peer_id'],
      registers: [this._registry],
    });

    this._accountDebitsTotal = new client.Counter({
      name: 'account_debits_total',
      help: 'Total debits applied to accounts',
      labelNames: ['peer_id'],
      registers: [this._registry],
    });

    // Initialize channel metrics
    this._activeChannels = new client.Gauge({
      name: 'payment_channels_active',
      help: 'Number of active payment channels',
      labelNames: ['method', 'status'],
      registers: [this._registry],
    });

    this._channelFundedTotal = new client.Counter({
      name: 'payment_channels_funded_total',
      help: 'Total number of channels funded',
      labelNames: ['method'],
      registers: [this._registry],
    });

    this._channelClosedTotal = new client.Counter({
      name: 'payment_channels_closed_total',
      help: 'Total number of channels closed',
      labelNames: ['method', 'reason'],
      registers: [this._registry],
    });

    this._channelDisputesTotal = new client.Counter({
      name: 'payment_channels_disputes_total',
      help: 'Total number of channel disputes',
      labelNames: ['method'],
      registers: [this._registry],
    });

    // Initialize error metrics
    this._errorsTotal = new client.Counter({
      name: 'connector_errors_total',
      help: 'Total number of connector errors',
      labelNames: ['type', 'severity'],
      registers: [this._registry],
    });

    this._lastErrorTimestamp = new client.Gauge({
      name: 'connector_last_error_timestamp',
      help: 'Timestamp of the last error',
      registers: [this._registry],
    });

    // Initialize claim metrics
    this._claimsSentTotal = new client.Counter({
      name: 'claims_sent_total',
      help: 'Total claims sent to peers',
      labelNames: ['peer_id', 'blockchain', 'success'],
      registers: [this._registry],
    });

    this._claimsReceivedTotal = new client.Counter({
      name: 'claims_received_total',
      help: 'Total claims received from peers',
      labelNames: ['peer_id', 'blockchain', 'verified'],
      registers: [this._registry],
    });

    this._claimsRedeemedTotal = new client.Counter({
      name: 'claims_redeemed_total',
      help: 'Total claims redeemed on-chain',
      labelNames: ['blockchain', 'success'],
      registers: [this._registry],
    });

    this._claimRedemptionLatencySeconds = new client.Histogram({
      name: 'claim_redemption_latency_seconds',
      help: 'Time from claim receipt to on-chain redemption',
      labelNames: ['blockchain'],
      buckets: CLAIM_REDEMPTION_LATENCY_BUCKETS,
      registers: [this._registry],
    });

    this._claimVerificationFailuresTotal = new client.Counter({
      name: 'claim_verification_failures_total',
      help: 'Total claim verification failures',
      labelNames: ['peer_id', 'blockchain', 'error_type'],
      registers: [this._registry],
    });

    this._claimLastRedemptionTimestampSeconds = new client.Gauge({
      name: 'claim_last_redemption_timestamp_seconds',
      help: 'Unix timestamp of last successful claim redemption',
      labelNames: ['blockchain'],
      registers: [this._registry],
    });

    this._logger.info({ config: this._config }, 'PrometheusExporter initialized');
  }

  /**
   * Record a processed ILP packet
   *
   * @param options - Packet metrics options
   */
  recordPacket(options: PacketMetricsOptions): void {
    const { type, status, latencyMs } = options;

    // Increment packet counter
    this._packetsProcessedTotal.inc({ type, status });

    // Record latency in seconds
    const latencySeconds = latencyMs / 1000;
    this._packetLatencySeconds.observe({ type }, latencySeconds);

    // Track for SLA calculation
    this._packetTotalCount++;
    if (status === 'success') {
      this._packetSuccessCount++;
    }
    this._latencySamples.push(latencyMs);

    // Limit latency samples to prevent memory bloat (keep last 10000)
    if (this._latencySamples.length > 10000) {
      this._latencySamples = this._latencySamples.slice(-10000);
    }

    this._logger.trace({ type, status, latencyMs }, 'Packet metrics recorded');
  }

  /**
   * Increment packets in flight counter
   */
  incrementPacketsInFlight(): void {
    this._packetsInFlight.inc();
  }

  /**
   * Decrement packets in flight counter
   */
  decrementPacketsInFlight(): void {
    this._packetsInFlight.dec();
  }

  /**
   * Record a settlement operation
   *
   * @param options - Settlement metrics options
   */
  recordSettlement(options: SettlementMetricsOptions): void {
    const { method, status, latencyMs, amount, tokenId } = options;

    // Increment settlement counter
    this._settlementsExecutedTotal.inc({ method, status });

    // Record latency in seconds
    const latencySeconds = latencyMs / 1000;
    this._settlementLatencySeconds.observe({ method }, latencySeconds);

    // Record amount if provided
    if (amount !== undefined && tokenId) {
      // Convert bigint to number for Prometheus (may lose precision for very large amounts)
      const amountNumber = Number(amount);
      this._settlementAmountTotal.inc({ method, token: tokenId }, amountNumber);
    }

    // Track for SLA calculation
    this._settlementTotalCount++;
    if (status === 'success') {
      this._settlementSuccessCount++;
    }

    this._logger.trace(
      { method, status, latencyMs, amount: amount?.toString() },
      'Settlement metrics recorded'
    );
  }

  /**
   * Update account balance gauge
   *
   * @param peerId - Peer identifier
   * @param tokenId - Token identifier
   * @param balance - Current balance in smallest units
   */
  updateAccountBalance(peerId: string, tokenId: string, balance: bigint): void {
    // Convert bigint to number for Prometheus gauge
    const balanceNumber = Number(balance);
    this._accountBalanceUnits.set({ peer_id: peerId, token_id: tokenId }, balanceNumber);

    this._logger.trace({ peerId, tokenId, balance: balance.toString() }, 'Account balance updated');
  }

  /**
   * Record account credit
   *
   * @param peerId - Peer identifier
   * @param amount - Credit amount
   */
  recordAccountCredit(peerId: string, amount: bigint): void {
    const amountNumber = Number(amount);
    this._accountCreditsTotal.inc({ peer_id: peerId }, amountNumber);
  }

  /**
   * Record account debit
   *
   * @param peerId - Peer identifier
   * @param amount - Debit amount
   */
  recordAccountDebit(peerId: string, amount: bigint): void {
    const amountNumber = Number(amount);
    this._accountDebitsTotal.inc({ peer_id: peerId }, amountNumber);
  }

  /**
   * Update active channel count
   *
   * @param method - Settlement method (evm)
   * @param status - Channel status
   * @param count - Number of channels in this state
   */
  updateActiveChannels(method: SettlementMethod, status: ChannelStatus, count: number): void {
    this._activeChannels.set({ method, status }, count);

    this._logger.trace({ method, status, count }, 'Active channels updated');
  }

  /**
   * Record a channel lifecycle event
   *
   * @param options - Channel metrics options
   */
  recordChannelEvent(options: ChannelMetricsOptions): void {
    const { method, event, reason } = options;

    switch (event) {
      case 'funded':
        this._channelFundedTotal.inc({ method });
        break;
      case 'closed':
        this._channelClosedTotal.inc({ method, reason: reason || 'normal' });
        break;
      case 'disputed':
        this._channelDisputesTotal.inc({ method });
        break;
    }

    this._logger.trace({ method, event, reason }, 'Channel event recorded');
  }

  /**
   * Record an error
   *
   * @param options - Error metrics options
   */
  recordError(options: ErrorMetricsOptions): void {
    const { type, severity } = options;

    this._errorsTotal.inc({ type, severity });
    this._lastErrorTimestamp.set(Date.now() / 1000);

    this._logger.trace({ type, severity }, 'Error metrics recorded');
  }

  /**
   * Record a claim sent event for Prometheus metrics
   *
   * @param options - Claim metrics options
   * @param options.blockchain - Blockchain type ('evm')
   * @param options.peerId - Peer identifier
   * @param options.success - Whether claim send was successful
   *
   * @example
   * ```typescript
   * exporter.recordClaimSent({
   *   blockchain: 'evm',
   *   peerId: 'peer-bob',
   *   success: true
   * });
   * ```
   */
  recordClaimSent(options: ClaimMetricsOptions): void {
    const { blockchain, peerId, success } = options;

    if (success === undefined) {
      this._logger.warn({ blockchain, peerId }, 'recordClaimSent called without success field');
      return;
    }

    this._claimsSentTotal.inc({
      peer_id: peerId,
      blockchain,
      success: success.toString(),
    });

    this._logger.trace({ blockchain, peerId, success }, 'Claim sent metrics recorded');
  }

  /**
   * Record a claim received event for Prometheus metrics
   *
   * @param options - Claim metrics options
   * @param options.blockchain - Blockchain type ('evm')
   * @param options.peerId - Peer identifier
   * @param options.verified - Whether claim was verified successfully
   *
   * @example
   * ```typescript
   * exporter.recordClaimReceived({
   *   blockchain: 'evm',
   *   peerId: 'peer-alice',
   *   verified: true
   * });
   * ```
   */
  recordClaimReceived(options: ClaimMetricsOptions): void {
    const { blockchain, peerId, verified } = options;

    if (verified === undefined) {
      this._logger.warn(
        { blockchain, peerId },
        'recordClaimReceived called without verified field'
      );
      return;
    }

    this._claimsReceivedTotal.inc({
      peer_id: peerId,
      blockchain,
      verified: verified.toString(),
    });

    this._logger.trace({ blockchain, peerId, verified }, 'Claim received metrics recorded');
  }

  /**
   * Record a claim redeemed event for Prometheus metrics
   *
   * @param options - Claim metrics options
   * @param options.blockchain - Blockchain type ('evm')
   * @param options.peerId - Peer identifier
   * @param options.success - Whether redemption was successful
   * @param options.latencyMs - Time from claim receipt to redemption in milliseconds (optional)
   *
   * @example
   * ```typescript
   * exporter.recordClaimRedeemed({
   *   blockchain: 'evm',
   *   peerId: 'peer-alice',
   *   success: true,
   *   latencyMs: 15000
   * });
   * ```
   */
  recordClaimRedeemed(options: ClaimMetricsOptions): void {
    const { blockchain, success, latencyMs } = options;

    if (success === undefined) {
      this._logger.warn({ blockchain }, 'recordClaimRedeemed called without success field');
      return;
    }

    this._claimsRedeemedTotal.inc({
      blockchain,
      success: success.toString(),
    });

    // Record latency if provided
    if (latencyMs !== undefined) {
      const latencySeconds = latencyMs / 1000;
      this._claimRedemptionLatencySeconds.observe({ blockchain }, latencySeconds);
    }

    // Update last redemption timestamp on successful redemption
    if (success) {
      this._claimLastRedemptionTimestampSeconds.set({ blockchain }, Date.now() / 1000);
    }

    this._logger.trace({ blockchain, success, latencyMs }, 'Claim redeemed metrics recorded');
  }

  /**
   * Record a claim verification failure for Prometheus metrics
   *
   * @param options - Claim metrics options
   * @param options.blockchain - Blockchain type ('evm')
   * @param options.peerId - Peer identifier
   * @param options.errorType - Error type ('invalid_signature', 'non_monotonic_nonce', 'unknown')
   *
   * @example
   * ```typescript
   * exporter.recordClaimVerificationFailure({
   *   blockchain: 'evm',
   *   peerId: 'peer-alice',
   *   errorType: 'invalid_signature'
   * });
   * ```
   */
  recordClaimVerificationFailure(options: ClaimMetricsOptions): void {
    const { blockchain, peerId, errorType } = options;

    if (!errorType) {
      this._logger.warn(
        { blockchain, peerId },
        'recordClaimVerificationFailure called without errorType'
      );
      return;
    }

    this._claimVerificationFailuresTotal.inc({
      peer_id: peerId,
      blockchain,
      error_type: errorType,
    });

    this._logger.trace({ blockchain, peerId, errorType }, 'Claim verification failure recorded');
  }

  /**
   * Get current SLA metrics
   *
   * @returns SLA metrics snapshot
   */
  getSLAMetrics(): SLAMetrics {
    const packetSuccessRate =
      this._packetTotalCount > 0 ? this._packetSuccessCount / this._packetTotalCount : 1.0;

    const settlementSuccessRate =
      this._settlementTotalCount > 0
        ? this._settlementSuccessCount / this._settlementTotalCount
        : 1.0;

    // Calculate p99 latency
    let p99LatencyMs = 0;
    if (this._latencySamples.length > 0) {
      const sorted = [...this._latencySamples].sort((a, b) => a - b);
      const p99Index = Math.floor(sorted.length * 0.99);
      p99LatencyMs = sorted[p99Index] || sorted[sorted.length - 1] || 0;
    }

    return {
      packetSuccessRate,
      settlementSuccessRate,
      p99LatencyMs,
    };
  }

  /**
   * Get metrics in Prometheus text format
   *
   * @returns Promise resolving to Prometheus-formatted metrics string
   */
  async getMetrics(): Promise<string> {
    return this._registry.metrics();
  }

  /**
   * Get content type for Prometheus metrics response
   *
   * @returns Content type string
   */
  getContentType(): string {
    return this._registry.contentType;
  }

  /**
   * Create Express middleware for serving metrics
   *
   * @returns Express request handler
   */
  getMetricsMiddleware(): RequestHandler {
    return async (_req: Request, res: Response): Promise<void> => {
      try {
        const metrics = await this.getMetrics();
        res.set('Content-Type', this.getContentType());
        res.send(metrics);
      } catch (error) {
        this._logger.error(
          { error: error instanceof Error ? error.message : 'Unknown error' },
          'Failed to collect metrics'
        );
        res.status(500).send('Failed to collect metrics');
      }
    };
  }

  /**
   * Reset all metrics (useful for testing)
   */
  reset(): void {
    this._registry.resetMetrics();
    this._packetSuccessCount = 0;
    this._packetTotalCount = 0;
    this._settlementSuccessCount = 0;
    this._settlementTotalCount = 0;
    this._latencySamples = [];

    this._logger.debug('Prometheus metrics reset');
  }

  /**
   * Clear the registry and unregister all metrics
   * Call this during shutdown to clean up
   */
  shutdown(): void {
    this._registry.clear();
    this._logger.info('PrometheusExporter shutdown complete');
  }

  /**
   * Get the underlying Prometheus registry
   * Useful for testing and advanced integrations
   *
   * @returns Prometheus Registry instance
   */
  getRegistry(): PromClient.Registry {
    return this._registry;
  }
}
