/**
 * MetricsCollector for Settlement Tracking
 *
 * Tracks settlement success and failure rates using a sliding window approach
 * to support circuit breaker logic and routing decisions.
 */

export interface MetricsConfig {
  slidingWindowDuration: number; // Duration in milliseconds (default: 3600000 = 1 hour)
  maxAttempts: number; // Maximum attempts to store per method (default: 1000)
  cleanupInterval: number; // Cleanup interval in milliseconds (default: 300000 = 5 minutes)
}

export interface SettlementAttempt {
  method: 'evm';
  success: boolean;
  timestamp: number;
}

export interface CircuitBreakerState {
  isOpen: boolean;
  failureRate: number;
}

/**
 * MetricsCollector tracks settlement success/failure rates for routing decisions
 */
export class MetricsCollector {
  private attempts: Map<string, SettlementAttempt[]>;
  private config: MetricsConfig;
  private cleanupIntervalId?: NodeJS.Timeout;

  constructor(config?: Partial<MetricsConfig>) {
    this.config = {
      slidingWindowDuration: config?.slidingWindowDuration ?? 3600000, // 1 hour
      maxAttempts: config?.maxAttempts ?? 1000,
      cleanupInterval: config?.cleanupInterval ?? 300000, // 5 minutes
    };

    this.attempts = new Map();

    // Start periodic cleanup
    this.cleanupIntervalId = setInterval(() => {
      this.cleanupExpiredAttempts();
    }, this.config.cleanupInterval);
  }

  /**
   * Record successful settlement
   */
  recordSuccess(method: 'evm'): void {
    this.recordAttempt(method, true);
  }

  /**
   * Record failed settlement
   */
  recordFailure(method: 'evm'): void {
    this.recordAttempt(method, false);
  }

  /**
   * Get overall success rate for a method (all time within sliding window)
   * Returns 1.0 if no attempts (assume healthy until proven otherwise)
   */
  getSuccessRate(method: string): number {
    const methodAttempts = this.attempts.get(method) ?? [];

    if (methodAttempts.length === 0) {
      return 1.0;
    }

    const successCount = methodAttempts.filter((a) => a.success).length;
    return successCount / methodAttempts.length;
  }

  /**
   * Get recent failure rate for circuit breaker logic
   * Returns 0.0 if no recent attempts
   */
  getRecentFailureRate(method: string): number {
    const now = Date.now();
    const windowStart = now - this.config.slidingWindowDuration;
    const methodAttempts = this.attempts.get(method) ?? [];

    const recentAttempts = methodAttempts.filter((a) => a.timestamp >= windowStart);

    if (recentAttempts.length === 0) {
      return 0.0;
    }

    const failureCount = recentAttempts.filter((a) => !a.success).length;
    return failureCount / recentAttempts.length;
  }

  /**
   * Get circuit breaker state for a method
   */
  getCircuitBreakerState(method: string): CircuitBreakerState {
    const failureRate = this.getRecentFailureRate(method);
    return {
      isOpen: failureRate > 0.1, // >10% failure rate opens breaker
      failureRate,
    };
  }

  /**
   * Clean up resources
   */
  destroy(): void {
    if (this.cleanupIntervalId) {
      clearInterval(this.cleanupIntervalId);
      this.cleanupIntervalId = undefined;
    }
  }

  /**
   * Record a settlement attempt
   */
  private recordAttempt(method: 'evm', success: boolean): void {
    const attempt: SettlementAttempt = {
      method,
      success,
      timestamp: Date.now(),
    };

    const methodAttempts = this.attempts.get(method) ?? [];
    methodAttempts.push(attempt);

    // Enforce max attempts limit
    if (methodAttempts.length > this.config.maxAttempts) {
      methodAttempts.shift(); // Remove oldest
    }

    this.attempts.set(method, methodAttempts);

    // Immediate cleanup after recording
    this.cleanupExpiredAttempts();
  }

  /**
   * Remove attempts older than the sliding window
   */
  private cleanupExpiredAttempts(): void {
    const now = Date.now();
    const windowStart = now - this.config.slidingWindowDuration;

    for (const [method, methodAttempts] of this.attempts.entries()) {
      const validAttempts = methodAttempts.filter((a) => a.timestamp >= windowStart);
      this.attempts.set(method, validAttempts);
    }
  }
}
