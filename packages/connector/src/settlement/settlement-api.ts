/**
 * Settlement API - HTTP API for triggering and monitoring settlement operations
 *
 * Provides RESTful HTTP endpoints for manual settlement triggering and status queries.
 * Integrates with SettlementMonitor for automatic settlement on threshold crossings.
 *
 * **IMPORTANT: This is a STUB implementation with MOCK settlement execution**
 * - Settlement transfers recorded to TigerBeetle (balance reduced)
 * - NO real blockchain transactions sent (Epic 7 will add real blockchain settlement)
 * - All settlement logs include "settlement_type=MOCK" tag
 * - 100ms delay simulates blockchain transaction time
 *
 * **API Endpoints:**
 * - POST /settlement/execute - Execute settlement for a peer
 * - GET /settlement/status/:peerId - Query current settlement state
 *
 * **Authentication:**
 * - Bearer token authentication via Authorization header
 * - Token configured via SETTLEMENT_AUTH_TOKEN environment variable
 * - Authentication optional for MVP (development mode)
 *
 * **Integration Points:**
 * - SettlementMonitor (Story 6.6): Listens for SETTLEMENT_REQUIRED events
 * - AccountManager (Story 6.3): Balance queries and settlement recording
 * - HealthServer: Mounts settlement router on existing Express server
 *
 * @packageDocumentation
 */

import type { Router, Request, Response, NextFunction } from 'express';
import type { Logger } from 'pino';
import { requireOptional } from '../utils/optional-require';
import type { AccountManager } from './account-manager';
import type { SettlementMonitor } from './settlement-monitor';
import { SettlementState, SettlementTriggerEvent } from '../config/types';

/**
 * Settlement API Configuration
 *
 * Dependencies required to create the settlement router.
 *
 * @property accountManager - AccountManager for balance queries and settlement recording
 * @property settlementMonitor - SettlementMonitor for state tracking
 * @property logger - Pino logger for structured logging
 * @property authToken - Optional bearer token for authentication (omit to disable auth)
 */
export interface SettlementAPIConfig {
  /**
   * AccountManager instance for balance operations
   * Used to query balances and record settlement transfers
   */
  accountManager: AccountManager;

  /**
   * SettlementMonitor instance for state management
   * Tracks settlement state (IDLE, PENDING, IN_PROGRESS)
   * Provides event listeners for automatic settlement
   */
  settlementMonitor: SettlementMonitor;

  /**
   * Pino logger for structured logging
   * Settlement API uses child logger with component: 'settlement-api'
   */
  logger: Logger;

  /**
   * Optional bearer token for authentication
   * If undefined or empty, authentication is disabled (development mode)
   * Production deployments MUST configure SETTLEMENT_AUTH_TOKEN
   */
  authToken?: string;

  /**
   * Default token ID for settlement operations.
   * Resolved from the on-chain ERC-20 symbol at startup (e.g. 'M2M', 'USDC').
   */
  defaultTokenId?: string;
}

/**
 * Settlement Execution Request Body
 *
 * Request format for POST /settlement/execute endpoint.
 *
 * @property peerId - Peer ID to settle with (required)
 * @property tokenId - Token type to settle (optional, defaults to resolved on-chain symbol)
 */
export interface ExecuteSettlementRequest {
  /**
   * Peer ID to settle with
   * Must match peer ID from connector configuration
   * Example: 'connector-a', 'peer-b'
   */
  peerId: string;

  /**
   * Token ID to settle
   * Defaults to the resolved on-chain token symbol if not provided
   * Example: 'M2M', 'USDC', 'BTC'
   */
  tokenId?: string;
}

/**
 * Settlement Execution Response
 *
 * Response format for POST /settlement/execute endpoint.
 * All bigint amounts converted to strings for JSON serialization.
 *
 * @property success - Settlement execution result (true on success)
 * @property peerId - Peer ID settled with
 * @property tokenId - Token ID settled
 * @property previousBalance - Balance before settlement (bigint as string)
 * @property newBalance - Balance after settlement (bigint as string)
 * @property settledAmount - Amount settled (bigint as string)
 * @property timestamp - Settlement execution timestamp (ISO 8601)
 */
export interface ExecuteSettlementResponse {
  /**
   * Settlement execution result
   * True if settlement completed successfully
   */
  success: boolean;

  /**
   * Peer ID settled with
   */
  peerId: string;

  /**
   * Token ID settled
   */
  tokenId: string;

  /**
   * Balance before settlement
   * BigInt converted to string for JSON serialization
   * Example: "1000"
   */
  previousBalance: string;

  /**
   * Balance after settlement
   * Should be "0" for successful settlement
   * BigInt converted to string for JSON serialization
   */
  newBalance: string;

  /**
   * Amount settled
   * Equal to previousBalance for full settlement
   * BigInt converted to string for JSON serialization
   */
  settledAmount: string;

  /**
   * Settlement execution timestamp
   * ISO 8601 format: "2026-01-03T12:00:00.000Z"
   */
  timestamp: string;
}

/**
 * Settlement Status Response
 *
 * Response format for GET /settlement/status/:peerId endpoint.
 *
 * @property peerId - Peer ID queried
 * @property tokenId - Token ID queried
 * @property currentBalance - Current creditBalance (bigint as string)
 * @property settlementState - Current settlement state (IDLE, SETTLEMENT_PENDING, SETTLEMENT_IN_PROGRESS)
 * @property timestamp - Query timestamp (ISO 8601)
 */
export interface SettlementStatusResponse {
  /**
   * Peer ID queried
   */
  peerId: string;

  /**
   * Token ID queried
   */
  tokenId: string;

  /**
   * Current creditBalance (how much peer owes us)
   * BigInt converted to string for JSON serialization
   */
  currentBalance: string;

  /**
   * Current settlement state
   * - IDLE: No settlement in progress
   * - SETTLEMENT_PENDING: Threshold exceeded, waiting for settlement
   * - SETTLEMENT_IN_PROGRESS: Settlement execution in progress
   */
  settlementState: SettlementState;

  /**
   * Query timestamp
   * ISO 8601 format: "2026-01-03T12:00:00.000Z"
   */
  timestamp: string;
}

/**
 * Error Response Format
 *
 * Standard error response for all endpoints (4xx, 5xx).
 *
 * @property error - Human-readable error message
 */
export interface ErrorResponse {
  /**
   * Human-readable error message
   * Example: "Invalid peerId", "Unauthorized: Bearer token required"
   */
  error: string;
}

/**
 * Execute Mock Settlement
 *
 * Executes mock settlement for a peer-token combination.
 * Reduces peer's creditBalance to zero by recording a settlement transfer to TigerBeetle.
 *
 * **Mock Settlement Execution Flow:**
 * 1. Mark settlement IN_PROGRESS (prevent duplicate triggers)
 * 2. Query balance before settlement
 * 3. Calculate settled amount (entire creditBalance)
 * 4. Mock blockchain delay (100ms simulates on-chain settlement)
 * 5. Record settlement transfer to TigerBeetle
 * 6. Query balance after settlement (verify reduction)
 * 7. Mark settlement IDLE (ready for next threshold crossing)
 *
 * **MOCK Settlement Explanation:**
 * - Settlement transfer recorded to TigerBeetle (balance reduced to zero)
 * - NO real blockchain transaction sent
 * - Epic 7 will replace this with real EVM settlement
 * - Same API endpoint, different implementation
 *
 * @param config - Settlement API configuration
 * @param peerId - Peer ID to settle with
 * @param tokenId - Token ID to settle
 * @returns Settlement execution result with balances before/after
 * @throws Error if settlement transfer fails or balance not reduced to zero
 *
 * @internal
 */
async function executeMockSettlement(
  config: SettlementAPIConfig,
  peerId: string,
  tokenId: string
): Promise<ExecuteSettlementResponse> {
  const logger = config.logger.child({
    component: 'settlement-execution',
    peerId,
    tokenId,
  });

  logger.info('Executing mock settlement');

  // Mark settlement in progress (prevent duplicate triggers)
  config.settlementMonitor.markSettlementInProgress(peerId, tokenId);

  try {
    // Get balance before settlement
    const balanceBefore = await config.accountManager.getAccountBalance(peerId, tokenId);
    const settledAmount = balanceBefore.creditBalance;

    logger.info({ settledAmount: settledAmount.toString() }, 'Settlement amount calculated');

    // Mock blockchain transaction delay (simulate on-chain settlement time)
    // Real settlement (Epic 7) will have 1-5 second delay for blockchain confirmation
    await new Promise((resolve) => setTimeout(resolve, 100));

    // Record settlement transfer to TigerBeetle
    // This reduces creditBalance to zero (peer's debt cleared)
    await config.accountManager.recordSettlement(peerId, tokenId, settledAmount);

    // Get balance after settlement (verify reduction)
    const balanceAfter = await config.accountManager.getAccountBalance(peerId, tokenId);

    // Verify balance reduced to zero
    if (balanceAfter.creditBalance !== 0n) {
      logger.error(
        {
          balanceBefore: balanceBefore.creditBalance.toString(),
          balanceAfter: balanceAfter.creditBalance.toString(),
        },
        'Settlement did not reduce balance to zero'
      );
      throw new Error('Settlement verification failed: balance not reduced to zero');
    }

    // Mark settlement completed (reset to IDLE state)
    config.settlementMonitor.markSettlementCompleted(peerId, tokenId);

    // Log successful settlement with MOCK tag
    logger.info(
      {
        settlement_type: 'MOCK',
        settledAmount: settledAmount.toString(),
        balanceBefore: balanceBefore.creditBalance.toString(),
        balanceAfter: balanceAfter.creditBalance.toString(),
      },
      'Mock settlement executed successfully'
    );

    // Log mock settlement explanation
    logger.warn(
      'MOCK: Settlement logged to TigerBeetle, but no real blockchain transaction sent (Epic 7)'
    );

    return {
      success: true,
      peerId,
      tokenId,
      previousBalance: balanceBefore.creditBalance.toString(),
      newBalance: balanceAfter.creditBalance.toString(),
      settledAmount: settledAmount.toString(),
      timestamp: new Date().toISOString(),
    };
  } catch (error) {
    // Settlement failed - do NOT mark as completed (leave in IN_PROGRESS or reset to PENDING)
    // SettlementMonitor will retry on next polling cycle
    logger.error(
      {
        error: error instanceof Error ? error.message : 'Unknown error',
      },
      'Mock settlement execution failed'
    );

    throw error;
  }
}

/**
 * Create Authentication Middleware
 *
 * Creates Express middleware for bearer token authentication.
 * Validates Authorization header with Bearer token format.
 *
 * **Authentication Flow:**
 * - Extract Authorization header
 * - Validate "Bearer <token>" format
 * - Compare token with configured authToken
 * - Return 401 if header missing, 403 if token invalid
 *
 * **Development Mode:**
 * - If authToken is undefined or empty, returns no-op middleware
 * - Authentication disabled for local development
 * - Production deployments MUST configure SETTLEMENT_AUTH_TOKEN
 *
 * @param authToken - Optional bearer token for authentication
 * @returns Express middleware function
 *
 * @internal
 */
function createAuthMiddleware(
  authToken?: string
): (req: Request, res: Response, next: NextFunction) => void | Response {
  // If auth token not configured, return no-op middleware (authentication disabled)
  if (!authToken || authToken.trim() === '') {
    return (_req: Request, _res: Response, next: NextFunction): void => {
      next();
    };
  }

  // Return authentication middleware
  return (req: Request, res: Response, next: NextFunction): void | Response => {
    const authHeader = req.headers.authorization;

    // Validate Authorization header exists
    if (!authHeader || !authHeader.startsWith('Bearer ')) {
      return res.status(401).json({
        error: 'Unauthorized: Bearer token required',
      } as ErrorResponse);
    }

    // Extract token (remove "Bearer " prefix)
    const token = authHeader.substring(7);

    // Validate token
    if (token !== authToken) {
      return res.status(403).json({
        error: 'Forbidden: Invalid token',
      } as ErrorResponse);
    }

    // Token valid, continue to route handler
    next();
  };
}

/**
 * Create Settlement Router
 *
 * Creates Express Router with settlement API endpoints.
 * Mounts authentication middleware and route handlers.
 *
 * **Endpoints:**
 * - POST /settlement/execute - Execute settlement
 * - GET /settlement/status/:peerId - Query settlement status
 *
 * **Integration:**
 * - Router mounted on HealthServer Express app
 * - Shares port with health check endpoint (default 8080)
 * - Uses JSON body parser middleware
 * - Applies authentication middleware if auth token configured
 *
 * **Automatic Settlement:**
 * - Listens for SettlementMonitor SETTLEMENT_REQUIRED events
 * - Executes mock settlement automatically on threshold crossing
 * - Handles settlement errors gracefully (logs error, allows retry)
 *
 * @param config - Settlement API configuration
 * @returns Express Router with settlement endpoints
 *
 * @example
 * ```typescript
 * const settlementRouter = createSettlementRouter({
 *   accountManager,
 *   settlementMonitor,
 *   logger,
 *   authToken: process.env.SETTLEMENT_AUTH_TOKEN
 * });
 *
 * // Mount on Express app
 * app.use(settlementRouter);
 *
 * // Now accepting:
 * // POST http://localhost:8080/settlement/execute
 * // GET http://localhost:8080/settlement/status/peer-a
 * ```
 */
export async function createSettlementRouter(config: SettlementAPIConfig): Promise<Router> {
  const { default: express } = await requireOptional<{ default: typeof import('express') }>(
    'express',
    'HTTP admin/health APIs'
  );
  const router = express.Router();
  const logger = config.logger.child({ component: 'settlement-api' });

  // Add JSON body parser middleware
  router.use(express.json());

  // Add authentication middleware (if auth token configured)
  router.use(createAuthMiddleware(config.authToken));

  // Log authentication status
  if (!config.authToken || config.authToken.trim() === '') {
    logger.warn('Settlement API authentication DISABLED (no SETTLEMENT_AUTH_TOKEN configured)');
  } else {
    logger.info('Settlement API authentication ENABLED');
  }

  // Listen for automatic settlement triggers from SettlementMonitor
  config.settlementMonitor.on('SETTLEMENT_REQUIRED', async (event: SettlementTriggerEvent) => {
    try {
      logger.info(
        {
          peerId: event.peerId,
          tokenId: event.tokenId,
          currentBalance: event.currentBalance.toString(),
          threshold: event.threshold.toString(),
        },
        'Automatic settlement triggered by threshold detection'
      );

      await executeMockSettlement(config, event.peerId, event.tokenId);
    } catch (error) {
      logger.error(
        {
          error: error instanceof Error ? error.message : 'Unknown error',
          peerId: event.peerId,
          tokenId: event.tokenId,
        },
        'Automatic settlement failed'
      );
      // Settlement state remains IN_PROGRESS
      // SettlementMonitor will retry on next polling cycle
    }
  });

  logger.info('SettlementMonitor attached to SettlementAPI, automatic settlement enabled');

  /**
   * POST /settlement/execute
   *
   * Execute settlement for a peer-token combination.
   *
   * **Request Body:**
   * ```json
   * {
   *   "peerId": "connector-a",
   *   "tokenId": "M2M"  // Optional, defaults to resolved on-chain symbol
   * }
   * ```
   *
   * **Success Response (200 OK):**
   * ```json
   * {
   *   "success": true,
   *   "peerId": "connector-a",
   *   "tokenId": "M2M",
   *   "previousBalance": "1000",
   *   "newBalance": "0",
   *   "settledAmount": "1000",
   *   "timestamp": "2026-01-03T12:00:00.000Z"
   * }
   * ```
   *
   * **Error Responses:**
   * - 400 Bad Request: Invalid peerId or tokenId
   * - 401 Unauthorized: Missing Authorization header
   * - 403 Forbidden: Invalid bearer token
   * - 500 Internal Server Error: Settlement execution failed
   */
  router.post('/settlement/execute', async (req: Request, res: Response): Promise<void> => {
    try {
      const { peerId, tokenId = config.defaultTokenId ?? 'M2M' } =
        req.body as ExecuteSettlementRequest;

      // Validate peerId
      if (!peerId || typeof peerId !== 'string') {
        res.status(400).json({
          error: 'Invalid peerId',
        } as ErrorResponse);
        return;
      }

      // Validate tokenId (if provided)
      if (tokenId && typeof tokenId !== 'string') {
        res.status(400).json({
          error: 'Invalid tokenId',
        } as ErrorResponse);
        return;
      }

      logger.info({ peerId, tokenId }, 'Settlement execution requested');

      // Execute mock settlement
      const result = await executeMockSettlement(config, peerId, tokenId);

      res.status(200).json(result);
    } catch (error) {
      logger.error(
        {
          error: error instanceof Error ? error.message : 'Unknown error',
        },
        'Settlement execution failed'
      );

      res.status(500).json({
        error: error instanceof Error ? error.message : 'Settlement execution failed',
      } as ErrorResponse);
    }
  });

  /**
   * GET /settlement/status/:peerId
   *
   * Query current settlement status for a peer.
   *
   * **Query Parameters:**
   * - tokenId: Token ID to query (optional, defaults to resolved on-chain symbol)
   *
   * **Success Response (200 OK):**
   * ```json
   * {
   *   "peerId": "connector-a",
   *   "tokenId": "M2M",
   *   "currentBalance": "500",
   *   "settlementState": "IDLE",
   *   "timestamp": "2026-01-03T12:00:00.000Z"
   * }
   * ```
   *
   * **Error Responses:**
   * - 400 Bad Request: Missing peerId
   * - 404 Not Found: Account not found
   * - 500 Internal Server Error: Balance query failed
   */
  router.get('/settlement/status/:peerId', async (req: Request, res: Response): Promise<void> => {
    try {
      const { peerId } = req.params;
      const tokenId = (req.query.tokenId as string) ?? config.defaultTokenId ?? 'M2M';

      // Validate peerId
      if (!peerId) {
        res.status(400).json({
          error: 'peerId required',
        } as ErrorResponse);
        return;
      }

      logger.debug({ peerId, tokenId }, 'Settlement status queried');

      // Query current balance
      const balance = await config.accountManager.getAccountBalance(peerId, tokenId);

      // Query settlement state
      const state = config.settlementMonitor.getSettlementState(peerId, tokenId);

      const response: SettlementStatusResponse = {
        peerId,
        tokenId,
        currentBalance: balance.creditBalance.toString(),
        settlementState: state,
        timestamp: new Date().toISOString(),
      };

      res.status(200).json(response);
    } catch (error) {
      logger.error(
        {
          error: error instanceof Error ? error.message : 'Unknown error',
        },
        'Settlement status query failed'
      );

      // Check if account not found (404)
      if (error instanceof Error && error.message.includes('not found')) {
        res.status(404).json({
          error: error.message,
        } as ErrorResponse);
        return;
      }

      res.status(500).json({
        error: error instanceof Error ? error.message : 'Settlement status query failed',
      } as ErrorResponse);
    }
  });

  return router;
}
