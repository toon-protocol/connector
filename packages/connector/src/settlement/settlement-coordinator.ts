/**
 * Settlement Coordinator
 *
 * Intelligent EVM settlement router with circuit breaker pattern.
 * Evaluates EVM settlement based on cost, success rate, and latency.
 * Uses circuit breaker to disable failing settlement methods.
 *
 * @module settlement/settlement-coordinator
 */

import type { Logger } from 'pino';
import type { PaymentChannelSDK } from './payment-channel-sdk';
import type { MetricsCollector } from './metrics-collector';
import type { PeerConfig } from './types';

/**
 * Settlement option evaluation result
 */
export interface SettlementOption {
  method: 'evm';
  chain?: string; // 'base-l2' | 'ethereum' | 'polygon'
  estimatedCost: bigint; // Gas cost in native token
  estimatedLatency: number; // Seconds
  successRate: number; // 0.0 - 1.0
  available: boolean;
}

/**
 * Settlement routing decision (structured logging)
 */
export interface SettlementRoutingDecision {
  peerId: string;
  tokenId: string;
  amount: bigint;
  selectedMethod: 'evm';
  selectedChain?: string;
  estimatedCost: bigint;
  estimatedLatency: number;
  allOptions: SettlementOption[];
  timestamp: number;
}

/**
 * SettlementCoordinator configuration
 */
export interface SettlementCoordinatorConfig {
  peerConfigs: Map<string, PeerConfig>;
  gasPriceCacheDuration?: number; // Default: 30000ms (30 seconds)
}

/**
 * SettlementCoordinator Class
 *
 * Intelligent router for multi-chain settlement with cost optimization,
 * fallback logic, and circuit breaker pattern.
 */
export class SettlementCoordinator {
  private gasPriceCache?: { price: bigint; timestamp: number };
  private readonly gasPriceCacheDuration: number;

  constructor(
    private readonly evmSDK: PaymentChannelSDK,
    private readonly metricsCollector: MetricsCollector,
    private readonly config: SettlementCoordinatorConfig,
    private readonly logger: Logger
  ) {
    this.gasPriceCacheDuration = config.gasPriceCacheDuration ?? 30000; // 30 seconds default
  }

  /**
   * Select optimal settlement method
   *
   * Evaluates all available settlement options and returns the best one
   * based on cost, success rate, and latency.
   *
   * @param peerId - Peer identifier
   * @param tokenId - Token identifier (ERC20 address)
   * @param amount - Amount to settle
   * @returns Optimal settlement option
   * @throws Error if no settlement methods available
   */
  async selectSettlementMethod(
    peerId: string,
    tokenId: string,
    amount: bigint
  ): Promise<SettlementOption> {
    this.logger.info({ peerId, tokenId, amount: amount.toString() }, 'Selecting settlement method');

    // Evaluate all possible options
    const allOptions = await this.evaluateOptions(peerId, tokenId, amount);

    // Filter available options
    const availableOptions = allOptions.filter((opt) => opt.available);

    if (availableOptions.length === 0) {
      this.logger.error({ peerId, tokenId, allOptions }, 'No available settlement methods');
      throw new Error('No available settlement methods');
    }

    // Find optimal option using scoring
    const optimal = availableOptions.reduce((best, current) =>
      this.calculateScore(current) > this.calculateScore(best) ? current : best
    );

    // Log routing decision
    this.logRoutingDecision(peerId, tokenId, amount, optimal, allOptions);

    return optimal;
  }

  /**
   * Execute settlement with automatic fallback
   *
   * Attempts settlement with primary method, falls back to alternative if primary fails.
   *
   * @param peerId - Peer identifier
   * @param tokenId - Token identifier
   * @param amount - Amount to settle
   * @throws Error if all settlement methods fail
   */
  async executeSettlementWithFallback(
    peerId: string,
    tokenId: string,
    amount: bigint
  ): Promise<void> {
    this.logger.info({ peerId, tokenId, amount: amount.toString() }, 'Executing settlement');

    // Select primary method
    const primary = await this.selectSettlementMethod(peerId, tokenId, amount);

    try {
      // Try primary settlement
      await this.executeSettlement(primary, peerId, tokenId, amount);
      this.metricsCollector.recordSuccess(primary.method);
      this.logger.info({ peerId, method: primary.method }, 'Settlement succeeded');
      return;
    } catch (error) {
      this.metricsCollector.recordFailure(primary.method);
      this.logger.warn(
        { peerId, primaryMethod: primary.method, error: (error as Error).message },
        'Primary settlement failed, trying fallback'
      );

      // Try fallback method
      const fallback = await this.selectFallbackMethod(peerId, tokenId, amount, primary);

      if (!fallback) {
        this.logger.error({ peerId, tokenId }, 'All settlement methods failed');
        throw new Error('All settlement methods failed');
      }

      try {
        await this.executeSettlement(fallback, peerId, tokenId, amount);
        this.metricsCollector.recordSuccess(fallback.method);
        this.logger.info(
          { peerId, fallbackMethod: fallback.method },
          'Fallback settlement succeeded'
        );
      } catch (fallbackError) {
        this.metricsCollector.recordFailure(fallback.method);
        this.logger.error({ peerId, tokenId }, 'All settlement methods failed');
        throw new Error('All settlement methods failed');
      }
    }
  }

  /**
   * Calculate weighted score for settlement option
   *
   * Scoring weights:
   * - Cost: 50%
   * - Success rate: 30%
   * - Latency: 20%
   *
   * @param option - Settlement option to score
   * @returns Numeric score (higher is better)
   */
  private calculateScore(option: SettlementOption): number {
    // Cost score (inverse - lower cost is better)
    // Normalize to prevent division by zero
    const costScore = 1 / (Number(option.estimatedCost) + 1);

    // Success rate score (0.0 - 1.0)
    const successScore = option.successRate;

    // Latency score (inverse - lower latency is better)
    const latencyScore = 1 / (option.estimatedLatency + 1);

    // Weighted sum
    return costScore * 0.5 + successScore * 0.3 + latencyScore * 0.2;
  }

  /**
   * Evaluate all settlement options
   *
   * Checks EVM options based on peer configuration and token type.
   *
   * @param peerId - Peer identifier
   * @param tokenId - Token identifier
   * @param amount - Amount to settle
   * @returns Array of settlement options
   */
  private async evaluateOptions(
    peerId: string,
    tokenId: string,
    amount: bigint
  ): Promise<SettlementOption[]> {
    const options: SettlementOption[] = [];

    // Get peer configuration
    const peerConfig = await this.getPeerConfig(peerId);

    const canUseEVM =
      peerConfig.settlementPreference === 'evm' || peerConfig.settlementPreference === 'both';

    // Evaluate EVM option
    if (canUseEVM && peerConfig.evmAddress) {
      try {
        const evmCost = await this.estimateEVMCost(tokenId, amount);
        const evmSuccessRate = this.metricsCollector.getSuccessRate('evm');
        const circuitBreakerOpen = this.circuitBreakerOpen('evm');

        options.push({
          method: 'evm',
          chain: 'base-l2',
          estimatedCost: evmCost,
          estimatedLatency: 3, // seconds
          successRate: evmSuccessRate,
          available: !circuitBreakerOpen,
        });
      } catch (error) {
        this.logger.warn({ error: (error as Error).message }, 'EVM cost estimation failed');
        options.push({
          method: 'evm',
          chain: 'base-l2',
          estimatedCost: 0n,
          estimatedLatency: 3,
          successRate: 0,
          available: false,
        });
      }
    }

    return options;
  }

  /**
   * Check if circuit breaker is open for a method
   *
   * Circuit breaker opens when recent failure rate >10%
   *
   * @param method - Settlement method
   * @returns True if circuit breaker is open (method disabled)
   */
  private circuitBreakerOpen(method: string): boolean {
    const state = this.metricsCollector.getCircuitBreakerState(method);

    // Log state changes
    if (state.isOpen) {
      this.logger.warn(
        { method, failureRate: state.failureRate },
        'Circuit breaker opened for settlement method'
      );
    }

    return state.isOpen;
  }

  /**
   * Estimate EVM settlement cost
   *
   * Queries Base L2 RPC for gas price and estimates transaction cost.
   *
   * @param _tokenId - Token identifier (unused in MVP)
   * @param _amount - Amount to settle (unused in MVP)
   * @returns Estimated cost in wei
   */
  private async estimateEVMCost(_tokenId: string, _amount: bigint): Promise<bigint> {
    // Check cache first
    const now = Date.now();
    if (this.gasPriceCache && now - this.gasPriceCache.timestamp < this.gasPriceCacheDuration) {
      const gasUnits = 50000n; // Typical channel claim transaction
      return this.gasPriceCache.price * gasUnits;
    }

    // Query RPC for gas price
    // Access provider through evmSDK (using type assertion to access private field)
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const provider = (this.evmSDK as any).provider;
    const feeData = await provider.getFeeData();
    const gasPrice = feeData.gasPrice ?? 1000000n; // Fallback to 1 gwei

    // Update cache
    this.gasPriceCache = { price: gasPrice, timestamp: now };

    // Estimate gas units for channel claim
    const gasUnits = 50000n;
    return gasPrice * gasUnits;
  }

  /**
   * Select fallback settlement method
   *
   * Finds alternative settlement method after primary failure.
   *
   * @param peerId - Peer identifier
   * @param tokenId - Token identifier
   * @param amount - Amount to settle
   * @param primary - Primary settlement option that failed
   * @returns Fallback option or null if none available
   */
  private async selectFallbackMethod(
    peerId: string,
    tokenId: string,
    amount: bigint,
    primary: SettlementOption
  ): Promise<SettlementOption | null> {
    const allOptions = await this.evaluateOptions(peerId, tokenId, amount);
    const alternatives = allOptions.filter((opt) => opt.method !== primary.method && opt.available);

    if (alternatives.length === 0) {
      return null;
    }

    // Return best alternative
    return alternatives.reduce((best, current) =>
      this.calculateScore(current) > this.calculateScore(best) ? current : best
    );
  }

  /**
   * Execute settlement using specified method
   *
   * Routes to appropriate SDK based on settlement method.
   *
   * @param option - Settlement option to execute
   * @param peerId - Peer identifier
   * @param tokenId - Token identifier
   * @param amount - Amount to settle
   */
  private async executeSettlement(
    option: SettlementOption,
    peerId: string,
    tokenId: string,
    amount: bigint
  ): Promise<void> {
    this.logger.info(
      { method: option.method, peerId, tokenId, amount: amount.toString() },
      'Settlement executed'
    );

    // Get peer config for addresses
    const peerConfig = await this.getPeerConfig(peerId);

    if (option.method === 'evm') {
      if (!peerConfig.evmAddress) {
        throw new Error(`Peer ${peerId} missing evmAddress for EVM settlement`);
      }
      // Execute EVM settlement (opens new channel with deposit)
      const settlementTimeout = 86400; // 24 hours
      await this.evmSDK.openChannel(peerConfig.evmAddress, tokenId, settlementTimeout, amount);
    }
  }

  /**
   * Log routing decision
   *
   * Structured logging of settlement routing decision.
   *
   * @param peerId - Peer identifier
   * @param tokenId - Token identifier
   * @param amount - Amount to settle
   * @param selected - Selected settlement option
   * @param allOptions - All evaluated options
   */
  private logRoutingDecision(
    peerId: string,
    tokenId: string,
    amount: bigint,
    selected: SettlementOption,
    allOptions: SettlementOption[]
  ): void {
    const decision: SettlementRoutingDecision = {
      peerId,
      tokenId,
      amount,
      selectedMethod: selected.method,
      selectedChain: selected.chain,
      estimatedCost: selected.estimatedCost,
      estimatedLatency: selected.estimatedLatency,
      allOptions: allOptions.map((opt) => ({
        method: opt.method,
        chain: opt.chain,
        available: opt.available,
        estimatedCost: opt.estimatedCost,
        successRate: opt.successRate,
        estimatedLatency: opt.estimatedLatency,
      })),
      timestamp: Date.now(),
    };

    this.logger.info(decision, 'Settlement routing decision');
  }

  /**
   * Get peer configuration
   *
   * Retrieves peer configuration from config map.
   *
   * @param peerId - Peer identifier
   * @returns Peer configuration
   * @throws Error if peer not found
   */
  private async getPeerConfig(peerId: string): Promise<PeerConfig> {
    const config = this.config.peerConfigs.get(peerId);
    if (!config) {
      throw new Error(`Peer not found: ${peerId}`);
    }
    return config;
  }
}
