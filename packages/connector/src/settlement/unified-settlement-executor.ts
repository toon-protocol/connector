/**
 * Unified Settlement Executor
 *
 * Routes settlement operations to EVM payment channels based on peer configuration and token type.
 *
 * This executor listens for SETTLEMENT_REQUIRED events from SettlementMonitor
 * and settles via PaymentChannelSDK (EVM payment channels - Epic 8).
 *
 * Settlement routing logic:
 * - ERC20 token + peer allows EVM → EVM settlement
 * - Incompatible combinations → Error
 *
 * Epic 17 Integration (BTP Off-Chain Claim Exchange):
 * - After signing claims, sends them to peers via BTP using ClaimSender
 * - Retrieves BTPClient instances from BTPClientManager for peer connections
 * - Handles claim send failures gracefully (logs error, allows retry)
 * - Settlement completes only after claim successfully delivered to peer
 *
 * @module settlement/unified-settlement-executor
 */

import type { Logger } from 'pino';
import type { PaymentChannelSDK } from './payment-channel-sdk';
import type { SettlementMonitor } from './settlement-monitor';
import type { AccountManager } from './account-manager';
import type { PeerConfig, SettlementRequiredEvent, UnifiedSettlementExecutorConfig } from './types';
import type { ClaimSender } from './claim-sender';
import type { BTPClientManager } from '../btp/btp-client-manager';
import type { BTPClient } from '../btp/btp-client';
import type { PerPacketClaimService } from './per-packet-claim-service';

/**
 * Error thrown when settlement is disabled via feature flag
 */
export class SettlementDisabledError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'SettlementDisabledError';
  }
}

/**
 * UnifiedSettlementExecutor Class
 *
 * Orchestrates EVM-only settlement routing.
 * Integrates with TigerBeetle accounting layer for unified balance tracking.
 */
export class UnifiedSettlementExecutor {
  private readonly boundHandleSettlement: (event: SettlementRequiredEvent) => Promise<void>;
  private readonly _claimSender: ClaimSender;
  private readonly _btpClientManager: BTPClientManager;
  private _perPacketClaimService: PerPacketClaimService | null = null;

  /**
   * Constructor - EVM-only settlement with Epic 17 claim exchange
   *
   * @param config - Unified settlement configuration with peer preferences
   * @param evmChannelSDK - PaymentChannelSDK for EVM settlements (Epic 8)
   * @param claimSender - ClaimSender for off-chain claim delivery via BTP (Epic 17)
   * @param btpClientManager - BTPClientManager for peer connection lookup (Epic 17)
   * @param settlementMonitor - Settlement monitor emitting SETTLEMENT_REQUIRED events
   * @param accountManager - TigerBeetle account manager for balance updates
   * @param logger - Pino logger instance
   */
  constructor(
    private config: UnifiedSettlementExecutorConfig,
    private evmChannelSDK: PaymentChannelSDK,
    claimSender: ClaimSender,
    btpClientManager: BTPClientManager,
    private settlementMonitor: SettlementMonitor,
    private accountManager: AccountManager,
    private logger: Logger
  ) {
    this._claimSender = claimSender;
    this._btpClientManager = btpClientManager;
    // Bind handler once in constructor (Event Listener Cleanup pattern)
    // This ensures same reference is used in both on() and off() calls
    this.boundHandleSettlement = this.handleSettlement.bind(this);
  }

  /**
   * Start settlement executor
   *
   * Registers listener for SETTLEMENT_REQUIRED events from SettlementMonitor.
   * Settlement routing begins after start() is called.
   */
  start(): void {
    this.logger.info('Starting UnifiedSettlementExecutor...');
    this.settlementMonitor.on('SETTLEMENT_REQUIRED', this.boundHandleSettlement);
    this.logger.info('UnifiedSettlementExecutor started');
  }

  /**
   * Add a peer's settlement configuration at runtime
   *
   * Stores the PeerConfig in the executor's peers Map for settlement routing.
   * Called by the Admin API when a peer is registered with settlement config.
   *
   * @param peerConfig - Settlement configuration for the peer
   */
  addPeerConfig(peerConfig: PeerConfig): void {
    this.config.peers.set(peerConfig.peerId, peerConfig);
    this.logger.info(
      { peerId: peerConfig.peerId, preference: peerConfig.settlementPreference },
      'Added peer settlement config'
    );
  }

  /**
   * Remove a peer's settlement configuration at runtime
   *
   * Removes the PeerConfig from the executor's peers Map.
   * Called by the Admin API when a peer is deleted.
   *
   * @param peerId - Peer identifier to remove
   * @returns true if the peer config existed and was removed
   */
  removePeerConfig(peerId: string): boolean {
    const existed = this.config.peers.delete(peerId);
    if (existed) {
      this.logger.info({ peerId }, 'Removed peer settlement config');
    }
    return existed;
  }

  /**
   * Get a peer's settlement configuration
   *
   * @param peerId - Peer identifier to look up
   * @returns PeerConfig if found, undefined otherwise
   */
  getPeerConfig(peerId: string): PeerConfig | undefined {
    return this.config.peers.get(peerId);
  }

  /**
   * Get all peer settlement configurations
   *
   * @returns Map of peerId to PeerConfig
   */
  getAllPeerConfigs(): Map<string, PeerConfig> {
    return this.config.peers;
  }

  /**
   * Stop settlement executor
   *
   * Unregisters listener and stops settlement processing.
   * Ensures proper cleanup of event handlers.
   */
  stop(): void {
    this.logger.info('Stopping UnifiedSettlementExecutor...');
    this.settlementMonitor.off('SETTLEMENT_REQUIRED', this.boundHandleSettlement);
    this.logger.info('UnifiedSettlementExecutor stopped');
  }

  /**
   * Set PerPacketClaimService for using latest per-packet claims in on-chain settlement
   * @param service - PerPacketClaimService instance
   */
  setPerPacketClaimService(service: PerPacketClaimService): void {
    this._perPacketClaimService = service;
    this.logger.info('PerPacketClaimService set for on-chain settlement');
  }

  /**
   * Get BTPClient instance for a peer (Epic 17)
   *
   * Retrieves active BTP connection for peer from BTPClientManager.
   * Validates connection state before returning client.
   *
   * @param peerId - Peer identifier
   * @returns BTPClient instance for peer
   * @throws Error if peer not connected or connection inactive
   */
  private getBTPClientForPeer(peerId: string): BTPClient {
    const client = this._btpClientManager.getClientForPeer(peerId);
    if (!client) {
      const error = `No BTP connection to peer ${peerId}`;
      this.logger.error({ peerId }, error);
      throw new Error(error);
    }

    if (!this._btpClientManager.isConnected(peerId)) {
      const error = `BTP connection to peer ${peerId} is not active`;
      this.logger.error({ peerId }, error);
      throw new Error(error);
    }

    return client;
  }

  /**
   * Handle settlement required event (private)
   *
   * Routes settlement to EVM method based on peer config and token type.
   * Updates TigerBeetle accounts after successful settlement.
   *
   * @param event - Settlement required event from SettlementMonitor
   * @throws Error if no compatible settlement method found
   */
  private async handleSettlement(event: SettlementRequiredEvent): Promise<void> {
    if (!this.config.enabled) {
      throw new SettlementDisabledError('Settlement is disabled');
    }

    const { peerId, balance, tokenId } = event;

    this.logger.info({ peerId, balance, tokenId }, 'Handling settlement request...');

    // Get peer configuration
    const peerConfig = this.config.peers.get(peerId);
    if (!peerConfig) {
      this.logger.error({ peerId }, 'Peer configuration not found');
      throw new Error(`Peer configuration not found for peerId: ${peerId}`);
    }

    // Route to EVM settlement
    try {
      // Normalize 'both' to 'any' for backward compatibility
      const preference =
        peerConfig.settlementPreference === 'both' ? 'any' : peerConfig.settlementPreference;

      // Check if EVM settlement is available
      const canUseEVM = preference === 'evm' || preference === 'any';

      if (!canUseEVM) {
        throw new Error(
          `No compatible settlement method for peer ${peerId} with token ${tokenId} (preference: ${preference})`
        );
      }

      // Route all tokens to EVM settlement
      await this.settleViaEVM(peerId, balance, tokenId, peerConfig);

      // Update TigerBeetle accounts (unified accounting layer)
      await this.accountManager.recordSettlement(peerId, tokenId, BigInt(balance));

      this.logger.info({ peerId, balance, tokenId }, 'Settlement completed successfully');
    } catch (error) {
      this.logger.error({ error, peerId, balance, tokenId }, 'Settlement failed');
      throw error;
    }
  }

  /**
   * Settle via EVM payment channels (private)
   *
   * Routes settlement to PaymentChannelSDK (Epic 8).
   * For MVP: Opens new channel with initial deposit for settlement.
   * Future: Channel reuse and cooperative settlement (deferred to future story).
   *
   * @param peerId - Peer identifier
   * @param amount - Amount to settle (string for bigint)
   * @param tokenAddress - ERC20 token contract address
   * @param config - Peer configuration
   */
  private async settleViaEVM(
    peerId: string,
    amount: string,
    tokenAddress: string,
    config: PeerConfig
  ): Promise<void> {
    this.logger.info({ peerId, amount, tokenAddress }, 'Settling via EVM payment channel...');

    if (!config.evmAddress) {
      throw new Error(`Peer ${peerId} missing evmAddress for EVM settlement`);
    }

    // For MVP: Open new channel with settlement amount as initial deposit
    // Default settlement timeout: 86400 seconds (24 hours)
    const settlementTimeout = 86400;
    const depositAmount = BigInt(amount);

    this.logger.info(
      {
        peerId,
        peerAddress: config.evmAddress,
        depositAmount: depositAmount.toString(),
        settlementTimeout,
      },
      'Opening new EVM payment channel for settlement...'
    );

    const { channelId } = await this.evmChannelSDK.openChannel(
      config.evmAddress,
      tokenAddress,
      settlementTimeout,
      depositAmount
    );

    // Sign balance proof for settlement amount
    const nonce = 1; // Initial nonce for new channel
    const signature = await this.evmChannelSDK.signBalanceProof(
      channelId,
      nonce,
      depositAmount,
      0n,
      '0x0000000000000000000000000000000000000000000000000000000000000000'
    );
    const signerAddress = await this.evmChannelSDK.getSignerAddress();

    // Send balance proof to peer via BTP (Epic 17)
    try {
      const btpClient = this.getBTPClientForPeer(peerId);

      // Obtain self-describing fields for Epic 31
      const chainId = await this.evmChannelSDK.getChainId();
      const tokenNetworkAddress = await this.evmChannelSDK.getTokenNetworkAddress(tokenAddress);

      const result = await this._claimSender.sendEVMClaim(
        peerId,
        btpClient,
        channelId,
        nonce,
        depositAmount.toString(),
        '0',
        '0x0000000000000000000000000000000000000000000000000000000000000000',
        signature,
        signerAddress,
        chainId,
        tokenNetworkAddress,
        tokenAddress
      );

      if (!result.success) {
        throw new Error(`Failed to send EVM claim to peer: ${result.error}`);
      }

      this.logger.info(
        {
          peerId,
          channelId,
          amount,
          messageId: result.messageId,
        },
        'EVM claim sent to peer successfully'
      );
    } catch (error) {
      this.logger.error({ error, peerId, channelId, amount }, 'Failed to send EVM claim');
      throw error;
    }

    // Reset per-packet claim tracking after settlement if available
    if (this._perPacketClaimService) {
      this._perPacketClaimService.resetChannel(channelId);
    }

    this.logger.info({ peerId, channelId, amount }, 'EVM settlement completed');
  }
}
