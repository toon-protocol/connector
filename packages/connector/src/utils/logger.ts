/**
 * Logger Configuration Module - Pino structured logging for ILP connector
 * @packageDocumentation
 * @remarks
 * Provides structured JSON logging with correlation IDs for packet tracking.
 * Outputs to stdout for Docker container log aggregation.
 */

import pino from 'pino';
import { randomBytes } from 'crypto';

/**
 * Logger type interface - wraps Pino logger
 * @remarks
 * Supports DEBUG, INFO, WARN, ERROR log levels with structured field logging.
 * Usage: logger.info({ field1, field2 }, 'message')
 */
export type Logger = pino.Logger;

/**
 * Valid log levels for the logger
 */
type LogLevel = 'debug' | 'info' | 'warn' | 'error';

/**
 * Default log level when LOG_LEVEL environment variable not set
 */
const DEFAULT_LOG_LEVEL: LogLevel = 'info';

/**
 * Validate and normalize log level from environment variable
 * @param envLevel - Log level from environment variable (case-insensitive)
 * @returns Normalized log level or default if invalid
 * @remarks
 * Converts to lowercase and validates against allowed values.
 * Returns default 'info' level if invalid value provided.
 */
function getValidLogLevel(envLevel?: string): LogLevel {
  if (!envLevel) {
    return DEFAULT_LOG_LEVEL;
  }

  const normalized = envLevel.toLowerCase();
  const validLevels: LogLevel[] = ['debug', 'info', 'warn', 'error'];

  if (validLevels.includes(normalized as LogLevel)) {
    return normalized as LogLevel;
  }

  return DEFAULT_LOG_LEVEL;
}

/**
 * Serializer for sanitizing wallet objects in logs
 * @param wallet - Wallet object (may contain sensitive data)
 * @returns Sanitized wallet object safe for logging
 * @remarks
 * Removes: privateKey, mnemonic, seed, encryptionKey, secret
 * CRITICAL: Prevents private key leakage in logs
 */
export function sanitizeWalletForLogs(wallet: Record<string, unknown>): Record<string, unknown> {
  if (!wallet || typeof wallet !== 'object') {
    return wallet;
  }

  // Create shallow copy to avoid mutating original
  const sanitized = { ...wallet };

  // Remove all sensitive fields
  sanitized.privateKey = '[REDACTED]';
  sanitized.mnemonic = '[REDACTED]';
  sanitized.seed = '[REDACTED]';
  sanitized.encryptionKey = '[REDACTED]';
  sanitized.secret = '[REDACTED]';

  // Also handle nested objects (e.g., wallet.signer.privateKey)
  if (sanitized.signer && typeof sanitized.signer === 'object') {
    sanitized.signer = { ...(sanitized.signer as Record<string, unknown>) };
    (sanitized.signer as Record<string, unknown>).privateKey = '[REDACTED]';
    (sanitized.signer as Record<string, unknown>).secret = '[REDACTED]';
  }

  return sanitized;
}

/**
 * Create configured Pino logger instance with node ID context
 * @param nodeId - Connector node ID to include in all log entries
 * @param logLevel - Optional log level override (defaults to LOG_LEVEL env var or 'info')
 * @returns Configured Pino logger instance with nodeId as base context
 *
 * @example
 * ```typescript
 * const logger = createLogger('connector-a');
 * logger.info({ correlationId: 'pkt_abc123', destination: 'g.dest' }, 'Packet received');
 * // Output: {"level":"info","time":1703620800000,"nodeId":"connector-a","correlationId":"pkt_abc123","destination":"g.dest","msg":"Packet received"}
 * ```
 *
 * @remarks
 * - Outputs JSON to stdout for Docker log aggregation
 * - Log level configurable via LOG_LEVEL environment variable (DEBUG, INFO, WARN, ERROR)
 * - Default level: INFO if LOG_LEVEL not set
 * - All log entries include nodeId field for multi-node differentiation
 * - Uses child logger pattern to inject nodeId context
 * - Wallet data serializers automatically redact sensitive cryptographic material
 */
export function createLogger(nodeId: string, logLevel?: string): Logger {
  // Get log level from parameter, environment variable, or default
  const level = logLevel ? getValidLogLevel(logLevel) : getValidLogLevel(process.env.LOG_LEVEL);

  // Configure serializers to redact sensitive wallet data
  const serializers = {
    wallet: sanitizeWalletForLogs,
    masterSeed: () => '[REDACTED]',
    privateKey: () => '[REDACTED]',
    mnemonic: () => '[REDACTED]',
    seed: () => '[REDACTED]',
    encryptionKey: () => '[REDACTED]',
    secret: () => '[REDACTED]',
  };

  // Create standard logger
  const baseLogger = pino({
    level,
    serializers,
  });

  // Return child logger with nodeId context
  // All logs from this logger will include nodeId field
  return baseLogger.child({ nodeId });
}

/**
 * Generate unique correlation ID for packet tracking
 * @returns Correlation ID in format: pkt_{16-character-hex-string}
 *
 * @example
 * ```typescript
 * const correlationId = generateCorrelationId();
 * // Returns: "pkt_abc123def4567890"
 * ```
 *
 * @remarks
 * Used to track ILP packets through multi-hop flows across log entries.
 * Format: 'pkt_' prefix + 16-character hex string from 8 random bytes.
 * Each call generates a unique ID using cryptographically secure randomness.
 */
export function generateCorrelationId(): string {
  return `pkt_${randomBytes(8).toString('hex')}`;
}
