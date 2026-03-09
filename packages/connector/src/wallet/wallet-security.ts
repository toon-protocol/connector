/**
 * Wallet Security Manager - Security hardening for agent wallets
 * Story 11.9: Security Hardening for Agent Wallets
 *
 * Implements key protection, spending limits, and fraud detection integration
 * for agent wallet infrastructure. Sanitizes sensitive data from logs/telemetry/APIs.
 */

import type { Logger } from 'pino';
import type Database from 'better-sqlite3';

/**
 * Spending limits configuration per agent
 * @remarks
 * Default limits: 1000 USDC per transaction, 5000 USDC daily, 50000 USDC monthly
 */
export interface SpendingLimits {
  maxTransactionSize: bigint; // Max single transaction (e.g., 1000 USDC)
  dailyLimit: bigint; // Max daily spending (e.g., 5000 USDC)
  monthlyLimit: bigint; // Max monthly spending (e.g., 50000 USDC)
}

/**
 * Security configuration for wallet operations
 */
export interface SecurityConfig {
  authentication: {
    method: 'password' | '2fa' | 'hsm'; // Authentication method
    passwordMinLength: number; // Minimum password length (default: 16)
    totpEnabled: boolean; // Enable TOTP 2FA (default: false)
  };
  rateLimits: {
    walletCreation: number; // Max wallet creations/hour (default: 100)
    fundingRequests: number; // Max funding requests/hour (default: 50)
  };
  spendingLimits: {
    default: SpendingLimits; // Default limits for all agents
    perAgent: Record<string, SpendingLimits>; // Custom limits per agent ID
  };
  fraudDetection: {
    rapidFundingThreshold: number; // Funding requests/hour before flagging (default: 5)
    unusualTransactionStdDev: number; // Std deviations from mean to flag (default: 3)
  };
}

/**
 * Fraud check result from Epic 12 fraud detector
 */
export interface FraudCheckResult {
  detected: boolean; // True if fraud detected
  reason?: string; // Human-readable fraud detection reason
  score?: number; // Fraud score (0-100, higher = more suspicious)
}

/**
 * Fraud detector interface (Epic 12 integration)
 */
export interface FraudDetector {
  analyzeTransaction(params: {
    agentId: string;
    amount: bigint;
    token: string;
    timestamp: number;
  }): Promise<FraudCheckResult>;
}

/**
 * Audit logger interface for wallet operations
 */
export interface AuditLogger {
  auditLog(operation: string, agentId: string, details: Record<string, unknown>): Promise<void>;
}

/**
 * Custom error for spending limit exceeded
 */
export class SpendingLimitExceededError extends Error {
  constructor(limitType: string, limit: bigint, attempted: bigint) {
    super(`${limitType} spending limit exceeded: ${attempted} > ${limit}`);
    this.name = 'SpendingLimitExceededError';
  }
}

/**
 * Wallet Security Manager
 * Handles key protection, spending limits, and fraud detection
 */
export class WalletSecurityManager {
  private config: SecurityConfig;
  private fraudDetector: FraudDetector;
  private logger: Logger;
  private db?: Database.Database; // SQLite database for transaction history (optional for MVP)

  constructor(
    config: SecurityConfig,
    fraudDetector: FraudDetector,
    logger: Logger,
    db?: Database.Database
  ) {
    this.config = config;
    this.fraudDetector = fraudDetector;
    this.logger = logger;
    this.db = db;
  }

  /**
   * Sanitize wallet data by removing all sensitive cryptographic material
   * @param wallet - Wallet object (may contain private keys, mnemonic, seed)
   * @returns Sanitized wallet object safe for logs/telemetry/API responses
   * @remarks
   * Removes: privateKey, mnemonic, seed, encryptionKey, secret
   * CRITICAL: Use this method before logging, emitting telemetry, or returning API responses
   */
  sanitizeWalletData<T>(wallet: T): T {
    if (!wallet || typeof wallet !== 'object') {
      return wallet;
    }

    // Create shallow copy to avoid mutating original
    const sanitized = { ...wallet } as Record<string, unknown>;

    // Remove all sensitive fields
    delete sanitized.privateKey;
    delete sanitized.mnemonic;
    delete sanitized.seed;
    delete sanitized.encryptionKey;
    delete sanitized.secret;

    // Also handle nested objects (e.g., wallet.signer.privateKey)
    if (sanitized.signer && typeof sanitized.signer === 'object') {
      sanitized.signer = { ...(sanitized.signer as Record<string, unknown>) };
      delete (sanitized.signer as Record<string, unknown>).privateKey;
      delete (sanitized.signer as Record<string, unknown>).secret;
    }

    return sanitized as T;
  }

  /**
   * Get spending limits for specified agent
   * @param agentId - Agent identifier
   * @returns Spending limits configuration
   * @remarks
   * Returns custom limits if configured, otherwise returns default limits
   */
  async getSpendingLimits(agentId: string): Promise<SpendingLimits> {
    // Check for custom agent-specific limits
    if (this.config.spendingLimits.perAgent[agentId]) {
      return this.config.spendingLimits.perAgent[agentId];
    }

    // Return default limits
    return this.config.spendingLimits.default;
  }

  /**
   * Get daily spending for agent
   * @param agentId - Agent identifier
   * @param token - Token symbol (e.g., 'USDC', 'DAI')
   * @returns Total spending in last 24 hours
   * @remarks
   * Queries transaction history from audit log
   * For MVP: Returns 0n if no database configured
   */
  async getDailySpending(agentId: string, token: string): Promise<bigint> {
    if (!this.db) {
      return 0n; // No database configured (MVP mode)
    }

    // Query transactions from last 24 hours
    const oneDayAgo = Date.now() - 24 * 60 * 60 * 1000;

    try {
      const stmt = this.db.prepare(`
        SELECT SUM(CAST(json_extract(details, '$.amount') AS INTEGER)) as total
        FROM wallet_audit_log
        WHERE agentId = ?
          AND operation = 'payment_sent'
          AND timestamp >= ?
          AND json_extract(details, '$.token') = ?
      `);

      const result = stmt.get(agentId, oneDayAgo, token) as { total: number | null } | undefined;
      return BigInt(result?.total ?? 0);
    } catch (error) {
      this.logger.warn({ error, agentId, token }, 'Failed to query daily spending');
      return 0n; // Return 0 on error (fail open for MVP)
    }
  }

  /**
   * Get monthly spending for agent
   * @param agentId - Agent identifier
   * @param token - Token symbol (e.g., 'USDC', 'DAI')
   * @returns Total spending in last 30 days
   * @remarks
   * Queries transaction history from audit log
   * For MVP: Returns 0n if no database configured
   */
  async getMonthlySpending(agentId: string, token: string): Promise<bigint> {
    if (!this.db) {
      return 0n; // No database configured (MVP mode)
    }

    // Query transactions from last 30 days
    const thirtyDaysAgo = Date.now() - 30 * 24 * 60 * 60 * 1000;

    try {
      const stmt = this.db.prepare(`
        SELECT SUM(CAST(json_extract(details, '$.amount') AS INTEGER)) as total
        FROM wallet_audit_log
        WHERE agentId = ?
          AND operation = 'payment_sent'
          AND timestamp >= ?
          AND json_extract(details, '$.token') = ?
      `);

      const result = stmt.get(agentId, thirtyDaysAgo, token) as
        | { total: number | null }
        | undefined;
      return BigInt(result?.total ?? 0);
    } catch (error) {
      this.logger.warn({ error, agentId, token }, 'Failed to query monthly spending');
      return 0n; // Return 0 on error (fail open for MVP)
    }
  }

  /**
   * Validate transaction against spending limits and fraud detection
   * @param agentId - Agent identifier
   * @param amount - Transaction amount
   * @param token - Token symbol (e.g., 'USDC', 'DAI')
   * @returns True if transaction valid, false if exceeds limits or fraud detected
   * @remarks
   * Checks: transaction size limit, daily limit, monthly limit, fraud detection
   */
  async validateTransaction(agentId: string, amount: bigint, token: string): Promise<boolean> {
    try {
      // Get spending limits for agent
      const limits = await this.getSpendingLimits(agentId);

      // Check transaction size limit
      if (amount > limits.maxTransactionSize) {
        this.logger.warn(
          {
            agentId,
            amount: amount.toString(),
            limit: limits.maxTransactionSize.toString(),
            token,
          },
          'Transaction exceeds max transaction size limit'
        );
        return false;
      }

      // Check daily spending limit
      const dailySpent = await this.getDailySpending(agentId, token);
      if (dailySpent + amount > limits.dailyLimit) {
        this.logger.warn(
          {
            agentId,
            amount: amount.toString(),
            dailySpent: dailySpent.toString(),
            dailyLimit: limits.dailyLimit.toString(),
            token,
          },
          'Transaction exceeds daily spending limit'
        );
        return false;
      }

      // Check monthly spending limit
      const monthlySpent = await this.getMonthlySpending(agentId, token);
      if (monthlySpent + amount > limits.monthlyLimit) {
        this.logger.warn(
          {
            agentId,
            amount: amount.toString(),
            monthlySpent: monthlySpent.toString(),
            monthlyLimit: limits.monthlyLimit.toString(),
            token,
          },
          'Transaction exceeds monthly spending limit'
        );
        return false;
      }

      // Check fraud detection (Epic 12 integration)
      const fraudCheck = await this.fraudDetector.analyzeTransaction({
        agentId,
        amount,
        token,
        timestamp: Date.now(),
      });

      if (fraudCheck.detected) {
        this.logger.error(
          {
            agentId,
            amount: amount.toString(),
            token,
            fraudReason: fraudCheck.reason,
            fraudScore: fraudCheck.score,
          },
          'Transaction flagged as fraudulent'
        );
        return false;
      }

      // All checks passed
      return true;
    } catch (error) {
      this.logger.error(
        { error, agentId, amount: amount.toString(), token },
        'Error validating transaction'
      );
      return false; // Fail closed on error
    }
  }
}
