/**
 * Account Manager for Double-Entry Peer Settlement
 *
 * Manages TigerBeetle account pairs for each peer connection, implementing
 * double-entry accounting with deterministic account IDs.
 *
 * Each peer-token combination has TWO accounts (duplex channel model):
 * - DEBIT account: Tracks amounts peer owes us (debited when receiving from peer)
 * - CREDIT account: Tracks amounts we owe peer (credited when forwarding to peer)
 *
 * Features:
 * - Deterministic account ID generation (idempotent account creation)
 * - In-memory cache for fast account ID lookups during packet forwarding
 * - Atomic account pair creation (both accounts created together)
 * - Account metadata encoding in TigerBeetle user_data fields
 * - Graceful duplicate account handling (safe retries)
 *
 * @module settlement/account-manager
 */

import { Logger } from 'pino';
import type { Transfer } from 'tigerbeetle-node';

// Local constants replacing runtime tigerbeetle-node SDK access.
// Both AccountFlags.none and TransferFlags.none are 0 in tigerbeetle-node.
const ACCOUNT_FLAGS_NONE = 0;
const TRANSFER_FLAGS_NONE = 0;

import type { ILedgerClient } from './ledger-client';
import { TigerBeetleAccountError } from './tigerbeetle-errors';
import { generateAccountId } from './account-id-generator';
import { encodeAccountMetadata } from './account-metadata';
import { CreditLimitConfig, CreditLimitViolation } from '../config/types';
import {
  AccountType,
  PeerAccountMetadata,
  PeerAccountPair,
  PeerAccountBalance,
  AccountLedgerCodes,
} from './types';
import {
  TigerBeetleBatchWriter,
  BatchWriterConfig,
  Transfer as BatchWriterTransfer,
  TransferError,
} from './tigerbeetle-batch-writer';

/**
 * Configuration for AccountManager initialization
 */
export interface AccountManagerConfig {
  /**
   * This connector's node ID (e.g., "connector-a")
   * Used to namespace account IDs for multi-node deployments
   */
  nodeId: string;

  /**
   * TigerBeetle ledger ID for all settlement accounts
   * Default: 1 (from AccountLedgerCodes.DEFAULT_LEDGER)
   * All peer accounts use the same ledger for MVP
   */
  defaultLedger?: number;

  /**
   * Optional credit limit configuration for managing counterparty risk
   * When provided, enforces limits on how much peers can owe the connector
   * Defaults to unlimited credit (no enforcement) if not specified
   */
  creditLimits?: CreditLimitConfig;

  /**
   * Optional batch writer configuration for high-throughput settlement (Story 12.5)
   * When provided, enables batched transfer writes for better performance
   * When not provided, uses direct synchronous writes (backward compatibility)
   */
  batchWriterConfig?: BatchWriterConfig;
}

/**
 * Account Manager
 *
 * Provides high-level account management operations for peer settlement:
 * - Create account pairs for peer connections
 * - Query account balances
 * - Cache account IDs for fast lookups
 *
 * This class abstracts the complexity of:
 * - Deterministic account ID generation
 * - Double-entry account pair management
 * - Metadata encoding
 * - Error handling and logging
 *
 * @example
 * const accountManager = new AccountManager(
 *   { nodeId: 'connector-a' },
 *   tigerBeetleClient,
 *   logger
 * );
 *
 * // Create accounts for peer connection
 * const accounts = await accountManager.createPeerAccounts('peer-b', 'USD');
 * // Returns: { debitAccountId: 123n, creditAccountId: 456n, peerId: 'peer-b', tokenId: 'USD' }
 *
 * // Query balances
 * const balance = await accountManager.getAccountBalance('peer-b', 'USD');
 * // Returns: { debitBalance: 1000n, creditBalance: 500n, netBalance: -500n }
 */
export class AccountManager {
  private readonly _config: Required<
    Omit<AccountManagerConfig, 'creditLimits' | 'batchWriterConfig'>
  >;
  private readonly _accountCache: Map<string, PeerAccountPair>;
  private readonly _confirmedAccounts: Set<string>; // Tracks accounts confirmed in TigerBeetle
  private readonly _creditLimitConfig: CreditLimitConfig | undefined;
  private readonly _batchWriter: TigerBeetleBatchWriter | undefined;

  constructor(
    config: AccountManagerConfig,
    private readonly _ledgerClient: ILedgerClient,
    private readonly _logger: Logger
  ) {
    this._config = {
      nodeId: config.nodeId,
      defaultLedger: config.defaultLedger ?? AccountLedgerCodes.DEFAULT_LEDGER,
    };

    this._creditLimitConfig = config.creditLimits;
    this._accountCache = new Map();
    this._confirmedAccounts = new Set();

    // Initialize batch writer if configured (Story 12.5)
    if (config.batchWriterConfig) {
      this._batchWriter = new TigerBeetleBatchWriter(
        config.batchWriterConfig,
        this._createTransferFn.bind(this),
        this._logger
      );

      this._logger.info(
        {
          batchSize: config.batchWriterConfig.batchSize,
          flushIntervalMs: config.batchWriterConfig.flushIntervalMs,
        },
        'AccountManager batch writer enabled for high-throughput settlement'
      );
    }

    // Log credit limit status at INFO level
    if (this._creditLimitConfig) {
      this._logger.info(
        {
          nodeId: this._config.nodeId,
          defaultLedger: this._config.defaultLedger,
          creditLimitsEnabled: true,
          defaultLimit: this._creditLimitConfig.defaultLimit?.toString(),
          globalCeiling: this._creditLimitConfig.globalCeiling?.toString(),
          batchingEnabled: !!this._batchWriter,
        },
        'AccountManager initialized with credit limits enabled'
      );
    } else {
      this._logger.info(
        {
          nodeId: this._config.nodeId,
          defaultLedger: this._config.defaultLedger,
          creditLimitsEnabled: false,
          batchingEnabled: !!this._batchWriter,
        },
        'AccountManager initialized (credit limits disabled - unlimited exposure)'
      );
    }
  }

  /**
   * Create account pair for a peer-token combination
   *
   * Creates both debit and credit accounts atomically in TigerBeetle.
   * Account IDs are deterministically generated, enabling idempotent creation.
   *
   * After successful creation, accounts are added to the in-memory cache for
   * fast lookups during packet forwarding operations.
   *
   * @param peerId - Peer connector ID (e.g., "connector-b")
   * @param tokenId - Token identifier (e.g., "USD", "ETH")
   * @returns Account pair with debit and credit account IDs
   * @throws {TigerBeetleAccountError} if account creation fails
   *
   * @example
   * const accounts = await accountManager.createPeerAccounts('peer-b', 'USD');
   * console.log(accounts);
   * // {
   * //   debitAccountId: 123456789012345678901234567890n,
   * //   creditAccountId: 987654321098765432109876543210n,
   * //   peerId: 'peer-b',
   * //   tokenId: 'USD'
   * // }
   */
  async createPeerAccounts(peerId: string, tokenId: string): Promise<PeerAccountPair> {
    // Generate deterministic account IDs
    const debitAccountId = generateAccountId(
      this._config.nodeId,
      peerId,
      tokenId,
      AccountType.DEBIT
    );

    const creditAccountId = generateAccountId(
      this._config.nodeId,
      peerId,
      tokenId,
      AccountType.CREDIT
    );

    this._logger.info(
      {
        peerId,
        tokenId,
        debitAccountId: debitAccountId.toString(),
        creditAccountId: creditAccountId.toString(),
      },
      'Creating peer account pair'
    );

    // Build account pair object (will be returned whether accounts are new or existing)
    const accountPair: PeerAccountPair = {
      debitAccountId,
      creditAccountId,
      peerId,
      tokenId,
    };

    try {
      // Build account objects with metadata encoding
      const debitAccount = await this._buildAccountObject(debitAccountId, AccountType.DEBIT, {
        nodeId: this._config.nodeId,
        peerId,
        tokenId,
        accountType: AccountType.DEBIT,
      });

      const creditAccount = await this._buildAccountObject(creditAccountId, AccountType.CREDIT, {
        nodeId: this._config.nodeId,
        peerId,
        tokenId,
        accountType: AccountType.CREDIT,
      });

      // Create both accounts atomically in a batch operation
      // This ensures either both accounts are created or neither
      await this._ledgerClient.createAccountsBatch([debitAccount, creditAccount]);

      this._logger.info(
        {
          peerId,
          tokenId,
          debitAccountId: debitAccountId.toString(),
          creditAccountId: creditAccountId.toString(),
        },
        'Peer account pair created successfully'
      );

      // Add to cache for fast lookups and mark as confirmed in TigerBeetle
      const cacheKey = this._getCacheKey(peerId, tokenId);
      this._accountCache.set(cacheKey, accountPair);
      this._confirmedAccounts.add(cacheKey);

      return accountPair;
    } catch (error) {
      // Handle duplicate account creation gracefully (idempotent operation)
      if (error instanceof TigerBeetleAccountError) {
        // Check if error message indicates accounts already exist
        const errorMessage = error.message.toLowerCase();
        if (
          errorMessage.includes('exists') ||
          errorMessage.includes('exists_with_different_flags') ||
          errorMessage.includes('linked_event_failed')
        ) {
          // Accounts already exist - this is expected with deterministic IDs
          // Log at INFO level (not an error - idempotent operation)
          this._logger.info(
            {
              peerId,
              tokenId,
              debitAccountId: debitAccountId.toString(),
              creditAccountId: creditAccountId.toString(),
            },
            'Accounts already exist for peer (idempotent operation)'
          );

          // Add to cache for future lookups and mark as confirmed
          const cacheKey = this._getCacheKey(peerId, tokenId);
          this._accountCache.set(cacheKey, accountPair);
          this._confirmedAccounts.add(cacheKey);

          // Return deterministically generated account pair
          // Since IDs are deterministic, we know these are the correct IDs
          return accountPair;
        }
      }

      // Other errors (validation failure, connection error, etc.) - re-throw
      this._logger.error(
        {
          error,
          peerId,
          tokenId,
          debitAccountId: debitAccountId.toString(),
          creditAccountId: creditAccountId.toString(),
        },
        'Failed to create peer account pair'
      );

      throw error;
    }
  }

  /**
   * Get account pair for a peer-token combination
   *
   * Retrieves account IDs from cache if available, otherwise generates
   * them deterministically. This method does NOT create accounts in TigerBeetle;
   * it only returns the account IDs that would be used.
   *
   * Use this method to get account IDs for balance queries or transfer operations
   * after accounts have been created with createPeerAccounts.
   *
   * @param peerId - Peer connector ID
   * @param tokenId - Token identifier
   * @returns Account pair with debit and credit account IDs
   *
   * @example
   * // Get account IDs (from cache or deterministic generation)
   * const accounts = accountManager.getPeerAccountPair('peer-b', 'USD');
   * console.log(accounts.debitAccountId);  // 123456789012345678901234567890n
   */
  getPeerAccountPair(peerId: string, tokenId: string): PeerAccountPair {
    const cacheKey = this._getCacheKey(peerId, tokenId);

    // Check cache first
    const cached = this._accountCache.get(cacheKey);
    if (cached) {
      this._logger.debug({ peerId, tokenId, source: 'cache' }, 'Retrieved account pair from cache');
      return cached;
    }

    // Generate deterministically if not cached
    const debitAccountId = generateAccountId(
      this._config.nodeId,
      peerId,
      tokenId,
      AccountType.DEBIT
    );

    const creditAccountId = generateAccountId(
      this._config.nodeId,
      peerId,
      tokenId,
      AccountType.CREDIT
    );

    const accountPair: PeerAccountPair = {
      debitAccountId,
      creditAccountId,
      peerId,
      tokenId,
    };

    // Add to cache for future lookups
    this._accountCache.set(cacheKey, accountPair);

    this._logger.debug(
      { peerId, tokenId, source: 'generated' },
      'Generated and cached account pair'
    );

    return accountPair;
  }

  /**
   * Ensure peer accounts exist in TigerBeetle
   *
   * Creates accounts if they don't exist (idempotent operation).
   * Uses cache to avoid unnecessary TigerBeetle calls for existing accounts.
   *
   * @param peerId - Peer connector ID
   * @param tokenId - Token identifier
   * @returns Account pair with debit and credit account IDs
   */
  async ensurePeerAccounts(peerId: string, tokenId: string): Promise<PeerAccountPair> {
    const cacheKey = this._getCacheKey(peerId, tokenId);

    // Only trust cache if accounts were confirmed created in TigerBeetle
    // (not just generated by getPeerAccountPair for read-only lookups)
    if (this._confirmedAccounts.has(cacheKey)) {
      const cached = this._accountCache.get(cacheKey);
      if (cached) {
        this._logger.debug(
          { peerId, tokenId, source: 'confirmed-cache' },
          'Account pair confirmed in TigerBeetle'
        );
        return cached;
      }
    }

    // Account not confirmed in TigerBeetle - create (idempotent)
    this._logger.debug(
      { peerId, tokenId },
      'Account pair not confirmed in TigerBeetle, creating accounts'
    );
    return await this.createPeerAccounts(peerId, tokenId);
  }

  /**
   * Query account balance for a peer-token combination
   *
   * Queries TigerBeetle for current balances of both debit and credit accounts,
   * and calculates the net balance.
   *
   * Balance interpretation:
   * - debitBalance > 0: Peer owes us (accounts receivable)
   * - creditBalance > 0: We owe peer (accounts payable)
   * - netBalance > 0: We need to settle TO peer
   * - netBalance < 0: Peer needs to settle TO us
   * - netBalance = 0: Balanced, no settlement needed
   *
   * @param peerId - Peer connector ID
   * @param tokenId - Token identifier
   * @returns Balance information for the peer-token combination
   * @throws {TigerBeetleAccountError} if balance query fails
   *
   * @example
   * const balance = await accountManager.getAccountBalance('peer-b', 'USD');
   * console.log(balance);
   * // {
   * //   debitBalance: 5000n,   // Peer owes us 5000
   * //   creditBalance: 3000n,  // We owe peer 3000
   * //   netBalance: -2000n     // Net: peer owes us 2000
   * // }
   */
  async getAccountBalance(peerId: string, tokenId: string): Promise<PeerAccountBalance> {
    // Get account IDs (from cache or generate)
    const accountPair = this.getPeerAccountPair(peerId, tokenId);

    this._logger.debug(
      {
        peerId,
        tokenId,
        debitAccountId: accountPair.debitAccountId.toString(),
        creditAccountId: accountPair.creditAccountId.toString(),
      },
      'Querying account balances'
    );

    try {
      // Query both accounts in a single batch operation
      const balances = await this._ledgerClient.getAccountsBatch([
        accountPair.debitAccountId,
        accountPair.creditAccountId,
      ]);

      // Extract balances (default to 0 if account not found)
      const debitAccountBalance = balances.get(accountPair.debitAccountId);
      const creditAccountBalance = balances.get(accountPair.creditAccountId);

      // Calculate balances
      // For debit account: balance = debits_posted - credits_posted
      // For credit account: balance = credits_posted - debits_posted
      const debitBalance = debitAccountBalance?.balance ?? 0n;
      const creditBalance = creditAccountBalance?.balance ?? 0n;

      // Net balance: positive = we owe peer, negative = peer owes us
      const netBalance = creditBalance - debitBalance;

      this._logger.debug(
        {
          peerId,
          tokenId,
          debitBalance: debitBalance.toString(),
          creditBalance: creditBalance.toString(),
          netBalance: netBalance.toString(),
        },
        'Account balances retrieved'
      );

      return {
        debitBalance,
        creditBalance,
        netBalance,
      };
    } catch (error) {
      this._logger.error(
        {
          error,
          peerId,
          tokenId,
        },
        'Failed to query account balances'
      );
      throw error;
    }
  }

  /**
   * Record settlement transfers for an ILP packet forward
   *
   * Creates two TigerBeetle transfers atomically:
   * 1. Incoming transfer: Debit fromPeer's credit account (peer owes us)
   * 2. Outgoing transfer: Credit toPeer's debit account (we owe peer)
   *
   * Both transfers are posted atomically - either both succeed or both fail.
   * This ensures double-entry accounting consistency.
   *
   * @param fromPeerId - Peer who sent us the packet
   * @param toPeerId - Peer we're forwarding to
   * @param tokenId - Token identifier (e.g., 'M2M')
   * @param incomingAmount - Original packet amount
   * @param outgoingAmount - Forwarded amount (after fee deduction)
   * @param transferId1 - Transfer ID for incoming transfer
   * @param transferId2 - Transfer ID for outgoing transfer
   * @param ledger - Ledger ID
   * @param code - Transfer code
   * @throws {TigerBeetleTransferError} if transfer creation fails
   *
   * @example
   * await accountManager.recordPacketTransfers(
   *   'peer-a', 'peer-b', 'M2M',
   *   1000n, 999n,
   *   transferId1, transferId2,
   *   1, 1
   * );
   */
  async recordPacketTransfers(
    fromPeerId: string,
    toPeerId: string,
    tokenId: string,
    incomingAmount: bigint,
    outgoingAmount: bigint,
    transferId1: bigint,
    transferId2: bigint,
    ledger: number,
    code: number
  ): Promise<void> {
    // Ensure accounts exist before recording transfers
    // createPeerAccounts is idempotent - safe to call multiple times
    const fromPeerAccounts = await this.ensurePeerAccounts(fromPeerId, tokenId);
    const toPeerAccounts = await this.ensurePeerAccounts(toPeerId, tokenId);

    this._logger.debug(
      {
        fromPeerId,
        toPeerId,
        tokenId,
        incomingAmount: incomingAmount.toString(),
        outgoingAmount: outgoingAmount.toString(),
      },
      'Recording packet settlement transfers'
    );

    // Build TWO transfers for atomic posting
    // In TigerBeetle double-entry:
    // - debit_account_id: account whose balance INCREASES (value removed from account)
    // - credit_account_id: account whose balance INCREASES (value added to account)
    //
    // Our account model:
    // - DEBIT account: Tracks peer owes us (accounts receivable)
    // - CREDIT account: Tracks we owe peer (accounts payable)
    const transfers: Transfer[] = [
      // Transfer 1 (Incoming): Peer sent us value
      // Debit the fromPeer's DEBIT account (increase "peer owes us")
      // Credit a placeholder account (future: connector revenue/clearing account)
      {
        id: transferId1,
        debit_account_id: fromPeerAccounts.debitAccountId, // Increase receivable
        credit_account_id: fromPeerAccounts.creditAccountId, // Temporary balancing (MVP)
        amount: incomingAmount,
        pending_id: 0n,
        user_data_128: 0n,
        user_data_64: 0n,
        user_data_32: 0,
        timeout: 0,
        ledger,
        code,
        flags: TRANSFER_FLAGS_NONE,
        timestamp: 0n,
      },
      // Transfer 2 (Outgoing): We forward value to next peer
      // Credit the toPeer's CREDIT account (increase "we owe peer")
      // Debit a placeholder account (future: connector revenue/clearing account)
      {
        id: transferId2,
        debit_account_id: toPeerAccounts.debitAccountId, // Temporary balancing (MVP)
        credit_account_id: toPeerAccounts.creditAccountId, // Increase payable
        amount: outgoingAmount,
        pending_id: 0n,
        user_data_128: 0n,
        user_data_64: 0n,
        user_data_32: 0,
        timeout: 0,
        ledger,
        code,
        flags: TRANSFER_FLAGS_NONE,
        timestamp: 0n,
      },
    ];

    // Post both transfers atomically
    await this._ledgerClient.createTransfersBatch(transfers);

    this._logger.info(
      {
        fromPeerId,
        toPeerId,
        incomingAmount: incomingAmount.toString(),
        outgoingAmount: outgoingAmount.toString(),
      },
      'Packet settlement transfers recorded successfully'
    );
  }

  /**
   * Check if proposed transfer would exceed credit limit
   *
   * Validates whether a proposed transfer amount would exceed the configured
   * credit limit for a peer-token combination. Returns null if transfer is
   * allowed (below limit or unlimited), or violation details if limit exceeded.
   *
   * Credit limit semantics:
   * - Credit limit applies to peer's debt to us (creditBalance from debit account)
   * - Limit represents maximum accounts receivable from peer
   * - Checks: (currentBalance + amount) <= effectiveLimit
   * - Effective limit = min(configuredLimit, globalCeiling)
   *
   * This method should be called BEFORE recording settlement transfers to
   * fail-safe prevent exceeding credit limits.
   *
   * @param peerId - Peer connector ID
   * @param tokenId - Token identifier
   * @param amount - Proposed transfer amount
   * @returns null if transfer allowed, CreditLimitViolation if limit exceeded
   *
   * @example
   * const violation = await accountManager.checkCreditLimit('peer-a', 'M2M', 1000n);
   * if (violation) {
   *   // Reject packet with T04_INSUFFICIENT_LIQUIDITY
   *   logger.warn({ violation }, 'Credit limit exceeded');
   *   return generateReject('T04', 'Credit limit exceeded');
   * }
   * // Proceed with settlement recording
   */
  async checkCreditLimit(
    peerId: string,
    tokenId: string,
    amount: bigint
  ): Promise<CreditLimitViolation | null> {
    // Get effective credit limit (with ceiling applied)
    const configuredLimit = this._getCreditLimitForPeer(peerId, tokenId);
    const limit = this._applyCeiling(configuredLimit);

    // If no limit configured, return null (unlimited - no violation)
    if (limit === undefined) {
      this._logger.debug(
        { peerId, tokenId, amount: amount.toString() },
        'Credit limit check: unlimited (no limit configured)'
      );
      return null;
    }

    // Get or create peer account pair
    let accountPair = this._accountCache.get(this._getCacheKey(peerId, tokenId));
    if (!accountPair) {
      // Account not in cache - create accounts first
      this._logger.debug(
        { peerId, tokenId },
        'Account pair not found in cache, creating accounts for credit limit check'
      );
      accountPair = await this.createPeerAccounts(peerId, tokenId);
    }

    // Query current balance from TigerBeetle
    const balance = await this.getAccountBalance(peerId, tokenId);

    // Calculate balance after proposed transfer
    // Credit balance = amount peer owes us (accounts receivable)
    // We're checking if peer's debt would exceed our limit
    const balanceAfter = balance.debitBalance + amount;

    // Check if balance after transfer would exceed limit
    if (balanceAfter <= limit) {
      // Transfer allowed - below or at limit
      this._logger.debug(
        {
          peerId,
          tokenId,
          currentBalance: balance.debitBalance.toString(),
          amount: amount.toString(),
          balanceAfter: balanceAfter.toString(),
          limit: limit.toString(),
        },
        'Credit limit check passed'
      );
      return null;
    }

    // Limit would be exceeded - create violation object
    const wouldExceedBy = balanceAfter - limit;
    const violation: CreditLimitViolation = {
      peerId,
      tokenId,
      currentBalance: balance.debitBalance,
      requestedAmount: amount,
      creditLimit: limit,
      wouldExceedBy,
    };

    // Log warning for credit limit violation
    this._logger.warn(
      {
        peerId,
        tokenId,
        currentBalance: balance.debitBalance.toString(),
        requestedAmount: amount.toString(),
        creditLimit: limit.toString(),
        wouldExceedBy: wouldExceedBy.toString(),
      },
      'Credit limit violation detected'
    );

    return violation;
  }

  /**
   * Convenience method to check if transfer would exceed credit limit
   *
   * Returns boolean instead of full violation object.
   * Useful for simple conditional checks.
   *
   * @param peerId - Peer connector ID
   * @param tokenId - Token identifier
   * @param amount - Proposed transfer amount
   * @returns true if limit would be exceeded, false otherwise
   *
   * @example
   * if (await accountManager.wouldExceedCreditLimit('peer-a', 'M2M', 1000n)) {
   *   // Reject packet
   * }
   */
  async wouldExceedCreditLimit(peerId: string, tokenId: string, amount: bigint): Promise<boolean> {
    const violation = await this.checkCreditLimit(peerId, tokenId, amount);
    return violation !== null;
  }

  /**
   * Clear the in-memory account cache
   *
   * Useful for testing or periodic cache refresh (future enhancement).
   * After clearing, account IDs will be regenerated deterministically on next access.
   */
  clearCache(): void {
    const cacheSize = this._accountCache.size;
    this._accountCache.clear();
    this._confirmedAccounts.clear();
    this._logger.info({ clearedEntries: cacheSize }, 'Account cache cleared');
  }

  /**
   * Get cache statistics for monitoring
   *
   * @returns Cache statistics including size
   */
  getCacheStats(): { size: number } {
    return {
      size: this._accountCache.size,
    };
  }

  /**
   * Shutdown the account manager and flush any pending batched transfers
   * (Story 12.5 - Batch Writer Integration)
   *
   * This method should be called during connector shutdown to ensure all
   * pending transfers are flushed before termination.
   *
   * @returns Promise that resolves when shutdown is complete
   */
  async shutdown(): Promise<void> {
    if (this._batchWriter) {
      this._logger.info('Shutting down AccountManager batch writer');
      await this._batchWriter.shutdown();
    }
  }

  /**
   * Get batch writer statistics (if batching is enabled)
   *
   * @returns Batch writer stats or undefined if batching is disabled
   */
  getBatchWriterStats():
    | {
        pendingTransfers: number;
        totalTransfersProcessed: number;
        totalBatchesFlushed: number;
        isFlushing: boolean;
      }
    | undefined {
    return this._batchWriter?.getStats();
  }

  /**
   * Record settlement transfer to TigerBeetle
   *
   * Records a settlement transfer that reduces peer's creditBalance (debt to us).
   * Settlement transfer moves value from peer's credit account to peer's debit account,
   * implementing bidirectional settlement (reduces both directions).
   *
   * **Settlement Transfer Logic:**
   * - Debit account: Peer's CREDIT account (reduce how much we owe peer)
   * - Credit account: Peer's DEBIT account (reduce how much peer owes us)
   * - Amount: Settlement amount (typically entire creditBalance)
   *
   * **Mock Settlement (Story 6.7):**
   * - Transfer recorded to TigerBeetle (balance reduced)
   * - NO real blockchain transaction sent (Epic 7 will add real settlement)
   * - Caller logs "settlement_type=MOCK" tag
   *
   * **Batching Behavior (Story 12.5):**
   * - If batch writer is enabled, transfer is queued and flushed asynchronously
   * - If batch writer is disabled, transfer is posted synchronously (backward compatibility)
   *
   * @param peerId - Peer connector ID
   * @param tokenId - Token identifier
   * @param amount - Settlement amount (bigint)
   * @returns Promise that resolves when transfer posted successfully
   * @throws {TigerBeetleAccountError} if transfer creation fails
   *
   * @example
   * // Record settlement for peer-a (settle entire balance)
   * const balance = await accountManager.getAccountBalance('peer-a', 'M2M');
   * await accountManager.recordSettlement('peer-a', 'M2M', balance.creditBalance);
   *
   * // Verify balance reduced to zero
   * const newBalance = await accountManager.getAccountBalance('peer-a', 'M2M');
   * console.log(newBalance.creditBalance); // 0n
   */
  async recordSettlement(peerId: string, tokenId: string, amount: bigint): Promise<void> {
    // Get peer account IDs
    const accountPair = this.getPeerAccountPair(peerId, tokenId);

    this._logger.debug(
      {
        peerId,
        tokenId,
        amount: amount.toString(),
        debitAccountId: accountPair.debitAccountId.toString(),
        creditAccountId: accountPair.creditAccountId.toString(),
        batchingEnabled: !!this._batchWriter,
      },
      'Recording settlement transfer'
    );

    // Generate unique transfer ID
    // Use timestamp-based ID for settlement transfers
    const transferId = BigInt(Date.now()) * 1000000n + BigInt(Math.floor(Math.random() * 1000000));

    // Create settlement transfer
    // In TigerBeetle double-entry accounting:
    // - debit_account_id: Account whose balance INCREASES (debited)
    // - credit_account_id: Account whose balance INCREASES (credited)
    //
    // Settlement transfer reduces creditBalance by:
    // - Debiting credit account (reduce debt we owe peer)
    // - Crediting debit account (reduce debt peer owes us)
    const transfer: BatchWriterTransfer = {
      id: transferId,
      debitAccountId: accountPair.creditAccountId, // Reduce our debt to peer
      creditAccountId: accountPair.debitAccountId, // Reduce peer's debt to us
      amount,
      ledger: this._config.defaultLedger,
      code: 1, // Settlement transfer code (distinguishes from packet transfers)
      flags: TRANSFER_FLAGS_NONE,
      userData128: 0n, // Future: Settlement metadata (settlement ID, blockchain tx hash)
      userData64: 0n, // Future: Settlement reason code
      userData32: 0,
      timeout: 0,
      timestamp: 0n,
    };

    try {
      if (this._batchWriter) {
        // Use batch writer for high-throughput settlement (Story 12.5)
        await this._batchWriter.addTransfer(transfer);
        this._logger.trace(
          {
            transferId: transferId.toString(),
            peerId,
            tokenId,
            amount: amount.toString(),
          },
          'Settlement transfer queued for batched write'
        );
      } else {
        // Direct synchronous write (backward compatibility)
        const tbTransfer: Transfer = this._convertToBatchWriterTransfer(transfer);
        await this._ledgerClient.createTransfersBatch([tbTransfer]);
        this._logger.debug(
          {
            transferId: transferId.toString(),
            peerId,
            tokenId,
            amount: amount.toString(),
          },
          'Settlement transfer recorded successfully (direct write)'
        );
      }
    } catch (error) {
      this._logger.error(
        {
          error: error instanceof Error ? error.message : 'Unknown error',
          peerId,
          tokenId,
          amount: amount.toString(),
        },
        'Settlement transfer failed'
      );

      // Wrap TigerBeetle error with context
      throw new TigerBeetleAccountError(
        `Settlement transfer failed for peer ${peerId}: ${
          error instanceof Error ? error.message : 'Unknown error'
        }`
      );
    }
  }

  /**
   * Build TigerBeetle account object with metadata encoding
   *
   * Creates a complete TigerBeetle account object with:
   * - Account ID (deterministically generated)
   * - Ledger and code based on account type
   * - Metadata encoded in user_data fields
   * - All balance fields initialized to zero
   *
   * @param accountId - Account ID (128-bit bigint)
   * @param accountType - Account type (DEBIT or CREDIT)
   * @param metadata - Peer account metadata to encode
   * @returns TigerBeetle account object ready for creation
   * @private
   */
  private async _buildAccountObject(
    accountId: bigint,
    accountType: AccountType,
    metadata: PeerAccountMetadata
  ): Promise<{
    id: bigint;
    ledger: number;
    code: number;
    flags: number;
    user_data_128: bigint;
    user_data_64: bigint;
    user_data_32: number;
  }> {
    // Encode metadata into user_data fields
    const encodedMetadata = encodeAccountMetadata(metadata);

    // Determine account code based on type
    const code =
      accountType === AccountType.DEBIT
        ? AccountLedgerCodes.ACCOUNT_CODE_PEER_DEBIT
        : AccountLedgerCodes.ACCOUNT_CODE_PEER_CREDIT;

    return {
      id: accountId,
      ledger: this._config.defaultLedger,
      code,
      flags: ACCOUNT_FLAGS_NONE,
      user_data_128: encodedMetadata.user_data_128,
      user_data_64: encodedMetadata.user_data_64,
      user_data_32: encodedMetadata.user_data_32,
    };
  }

  /**
   * Get credit limit for a peer-token combination
   *
   * Implements credit limit hierarchy (highest priority first):
   * 1. Token-specific limit: perTokenLimits[peerId][tokenId]
   * 2. Per-peer limit: perPeerLimits[peerId]
   * 3. Default limit: defaultLimit
   * 4. Unlimited: undefined
   *
   * @param peerId - Peer connector ID
   * @param tokenId - Token identifier
   * @returns Credit limit as bigint, or undefined for unlimited
   * @private
   */
  private _getCreditLimitForPeer(peerId: string, tokenId: string): bigint | undefined {
    // If credit limits not configured, return undefined (unlimited)
    if (!this._creditLimitConfig) {
      return undefined;
    }

    // Priority 1: Check token-specific limit
    const tokenSpecificLimit = this._creditLimitConfig.perTokenLimits?.get(peerId)?.get(tokenId);
    if (tokenSpecificLimit !== undefined) {
      return tokenSpecificLimit;
    }

    // Priority 2: Check per-peer limit
    const perPeerLimit = this._creditLimitConfig.perPeerLimits?.get(peerId);
    if (perPeerLimit !== undefined) {
      return perPeerLimit;
    }

    // Priority 3: Use default limit (may be undefined for unlimited)
    return this._creditLimitConfig.defaultLimit;
  }

  /**
   * Apply global credit limit ceiling to a configured limit
   *
   * Applies global ceiling as security override to prevent misconfiguration.
   * Returns minimum of configured limit and global ceiling.
   *
   * @param limit - Configured credit limit (may be undefined for unlimited)
   * @returns Effective limit after ceiling applied
   * @private
   *
   * @example
   * // Per-peer limit = 10000, global ceiling = 5000 → effective = 5000
   * const effective = this._applyCeiling(10000n);
   *
   * // Per-peer limit = 2000, global ceiling = 5000 → effective = 2000
   * const effective = this._applyCeiling(2000n);
   *
   * // No limit configured → effective = undefined (unlimited)
   * const effective = this._applyCeiling(undefined);
   */
  private _applyCeiling(limit: bigint | undefined): bigint | undefined {
    // If no limit configured, return undefined (unlimited)
    if (limit === undefined) {
      return undefined;
    }

    // If no global ceiling configured, return limit as-is
    if (!this._creditLimitConfig?.globalCeiling) {
      return limit;
    }

    // Return minimum of limit and global ceiling
    return limit < this._creditLimitConfig.globalCeiling
      ? limit
      : this._creditLimitConfig.globalCeiling;
  }

  /**
   * Generate cache key for peer-token combination
   *
   * @param peerId - Peer connector ID
   * @param tokenId - Token identifier
   * @returns Cache key string
   * @private
   */
  private _getCacheKey(peerId: string, tokenId: string): string {
    return `${peerId}:${tokenId}`;
  }

  /**
   * Transfer creation function for TigerBeetleBatchWriter (Story 12.5)
   *
   * This function is passed to the batch writer and called when a batch is flushed.
   * It converts batch writer transfers to TigerBeetle transfers and posts them.
   *
   * @param transfers - Array of batch writer transfers to post
   * @returns Array of transfer errors (empty if all succeeded)
   * @private
   */
  private async _createTransferFn(transfers: BatchWriterTransfer[]): Promise<TransferError[]> {
    // Convert batch writer transfers to TigerBeetle transfers
    const tbTransfers: Transfer[] = transfers.map((t) => this._convertToBatchWriterTransfer(t));

    try {
      // Post transfers to TigerBeetle
      await this._ledgerClient.createTransfersBatch(tbTransfers);
      return []; // All transfers succeeded
    } catch (error) {
      // TigerBeetle error - convert to transfer errors
      // For now, return generic error for all transfers
      // Future enhancement: Parse TigerBeetle error details for per-transfer errors
      this._logger.error(
        {
          error: error instanceof Error ? error.message : 'Unknown error',
          transferCount: transfers.length,
        },
        'Batch transfer creation failed'
      );

      return transfers.map((_, index) => ({
        index,
        code: 1, // Generic error code
      }));
    }
  }

  /**
   * Convert batch writer transfer to TigerBeetle transfer format
   *
   * @param transfer - Batch writer transfer
   * @returns TigerBeetle transfer
   * @private
   */
  private _convertToBatchWriterTransfer(transfer: BatchWriterTransfer): Transfer {
    return {
      id: transfer.id,
      debit_account_id: transfer.debitAccountId,
      credit_account_id: transfer.creditAccountId,
      amount: transfer.amount,
      pending_id: 0n,
      ledger: transfer.ledger,
      code: transfer.code,
      flags: transfer.flags,
      user_data_128: transfer.userData128 ?? 0n,
      user_data_64: transfer.userData64 ?? 0n,
      user_data_32: transfer.userData32 ?? 0,
      timeout: transfer.timeout ?? 0,
      timestamp: transfer.timestamp ?? 0n,
    };
  }
}
