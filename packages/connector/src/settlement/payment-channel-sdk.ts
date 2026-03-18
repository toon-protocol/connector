/**
 * Payment Channel SDK for Off-Chain Operations
 * Source: Epic 8 Story 8.7 - Off-Chain Payment Channel SDK
 *
 * This SDK wraps ethers.js for Base L2 blockchain interactions with TokenNetwork contracts.
 * Supports opening channels, signing EIP-712 balance proofs, closing channels, settling channels,
 * querying on-chain state, and listening to on-chain events.
 */

import type { Provider, Signer, Contract, Listener, Log, LogDescription, EventLog } from 'ethers';
import type {
  ChannelState,
  BalanceProof,
  ChannelOpenedEvent,
  ChannelClosedEvent,
  ChannelSettledEvent,
  ChannelCooperativeSettledEvent,
} from '@toon-protocol/shared';
import { getDomainSeparator, getBalanceProofTypes } from './eip712-helper';
import type { Logger } from '../utils/logger';
import type { KeyManager } from '../security/key-manager';
import { createKeyManagerSigner } from '../security/key-manager-signer';
import type { EVMRPCConnectionPool } from '../utils/evm-rpc-connection-pool';
import { requireOptional } from '../utils/optional-require';

// TokenNetworkRegistry ABI - only methods we need
const REGISTRY_ABI = [
  'function createTokenNetwork(address token) external returns (address)',
  'function getTokenNetwork(address token) external view returns (address)',
  'event TokenNetworkCreated(address indexed token, address indexed tokenNetwork)',
];

// TokenNetwork ABI - only methods/events we need
const TOKEN_NETWORK_ABI = [
  'function openChannel(address participant2, uint256 settlementTimeout) external returns (bytes32)',
  'function setTotalDeposit(bytes32 channelId, address participant, uint256 totalDeposit) external',
  'function closeChannel(bytes32 channelId) external',
  'function claimFromChannel(bytes32 channelId, tuple(bytes32 channelId, uint256 nonce, uint256 transferredAmount, uint256 lockedAmount, bytes32 locksRoot) balanceProof, bytes signature) external',
  'function settleChannel(bytes32 channelId) external',
  'function channels(bytes32) external view returns (uint256 settlementTimeout, uint8 state, uint256 closedAt, uint256 openedAt, address participant1, address participant2)',
  'function participants(bytes32, address) external view returns (uint256 deposit, uint256 nonce, uint256 transferredAmount)',
  'function claimedAmounts(bytes32, address) external view returns (uint256)',
  'event ChannelOpened(bytes32 indexed channelId, address indexed participant1, address indexed participant2, uint256 settlementTimeout)',
  'event ChannelClosed(bytes32 indexed channelId, address indexed closingParticipant)',
  'event ChannelSettled(bytes32 indexed channelId, uint256 participant1Amount, uint256 participant2Amount)',
  'event ChannelClaimed(bytes32 indexed channelId, address indexed claimant, uint256 claimedAmount, uint256 totalClaimed)',
];

// Standard ERC20 ABI for approvals, allowance checks, and symbol queries
const ERC20_ABI = [
  'function approve(address spender, uint256 amount) external returns (bool)',
  'function allowance(address owner, address spender) external view returns (uint256)',
  'function symbol() external view returns (string)',
];

/**
 * Custom error for challenge period not expired
 */
export class ChallengeNotExpiredError extends Error {
  constructor(
    message: string,
    public readonly channelId: string,
    public readonly closedAt: number,
    public readonly settlementTimeout: number
  ) {
    super(message);
    this.name = 'ChallengeNotExpiredError';
  }
}

/**
 * Payment Channel SDK Class
 * Manages off-chain payment channel operations with on-chain TokenNetwork contracts
 */
export class PaymentChannelSDK {
  private provider: Provider;
  private signer: Signer | null = null;
  private keyManager: KeyManager;
  private evmKeyId: string;
  private registryAddress: string;
  private registryContract: Contract | null = null;
  private tokenNetworkCache: Map<string, Contract>; // token address → TokenNetwork contract
  private channelStateCache: Map<string, ChannelState>; // channelId → channel state
  private logger: Logger;
  private eventListeners: Map<string, Array<Listener>>; // Track event listeners for cleanup
  private _initPromise: Promise<void> | null = null;

  /**
   * Create a new PaymentChannelSDK instance
   *
   * @param provider - Ethers.js provider for blockchain queries
   * @param keyManager - KeyManager for secure key operations (EIP-712 signing and transaction signing)
   * @param evmKeyId - EVM key identifier for KeyManager (backend-specific format)
   * @param registryAddress - TokenNetworkRegistry contract address
   * @param logger - Pino logger instance
   */
  constructor(
    provider: Provider,
    keyManager: KeyManager,
    evmKeyId: string,
    registryAddress: string,
    logger: Logger
  ) {
    this.provider = provider;
    this.keyManager = keyManager;
    this.evmKeyId = evmKeyId;
    this.registryAddress = registryAddress;
    this.tokenNetworkCache = new Map();
    this.channelStateCache = new Map();
    this.logger = logger;
    this.eventListeners = new Map();
  }

  /**
   * Lazily initialize signer and registry contract (requires async ethers import)
   */
  private async _ensureInitialized(): Promise<{
    signer: Signer;
    registryContract: Contract;
    ethers: (typeof import('ethers'))['ethers'];
  }> {
    if (this.signer && this.registryContract) {
      const ethersModule = await requireOptional<typeof import('ethers')>(
        'ethers',
        'EVM settlement'
      );
      return {
        signer: this.signer,
        registryContract: this.registryContract,
        ethers: ethersModule.ethers,
      };
    }

    if (!this._initPromise) {
      this._initPromise = (async () => {
        const { ethers: ethersNs } = await requireOptional<typeof import('ethers')>(
          'ethers',
          'EVM settlement'
        );
        this.signer = await createKeyManagerSigner(this.keyManager, this.evmKeyId, this.provider);
        this.registryContract = new ethersNs.Contract(
          this.registryAddress,
          REGISTRY_ABI,
          this.signer
        );
      })();
    }

    await this._initPromise;
    const ethersModule = await requireOptional<typeof import('ethers')>('ethers', 'EVM settlement');
    return {
      signer: this.signer!,
      registryContract: this.registryContract!,
      ethers: ethersModule.ethers,
    };
  }

  /**
   * Create a PaymentChannelSDK instance from a connection pool
   *
   * Uses the connection pool for failover and load balancing across multiple RPC endpoints.
   * For high-throughput scenarios, consider creating multiple SDK instances from the pool.
   *
   * @param pool - EVM RPC connection pool
   * @param keyManager - KeyManager for secure key operations
   * @param evmKeyId - EVM key identifier for KeyManager
   * @param registryAddress - TokenNetworkRegistry contract address
   * @param logger - Pino logger instance
   * @returns PaymentChannelSDK instance
   * @throws Error if no healthy connection available in pool
   *
   * [Source: Epic 12 Story 12.5 Task 6.4 - Connection pool integration]
   */
  static fromConnectionPool(
    pool: EVMRPCConnectionPool,
    keyManager: KeyManager,
    evmKeyId: string,
    registryAddress: string,
    logger: Logger
  ): PaymentChannelSDK {
    const provider = pool.getProvider();
    if (!provider) {
      throw new Error('No healthy EVM RPC connection available in pool');
    }

    logger.info('Creating PaymentChannelSDK from connection pool');
    return new PaymentChannelSDK(
      provider as Provider,
      keyManager,
      evmKeyId,
      registryAddress,
      logger
    );
  }

  /**
   * Get TokenNetwork contract for a given token address
   * Uses cache to avoid repeated registry lookups
   *
   * @param tokenAddress - ERC20 token address
   * @returns TokenNetwork contract instance
   */
  private async getTokenNetworkContract(tokenAddress: string): Promise<Contract> {
    // Check cache first
    if (this.tokenNetworkCache.has(tokenAddress)) {
      return this.tokenNetworkCache.get(tokenAddress)!;
    }

    const { signer, registryContract, ethers } = await this._ensureInitialized();

    // Query registry for TokenNetwork address
    const networkAddress = await registryContract.getTokenNetwork!(tokenAddress);
    if (networkAddress === ethers.ZeroAddress) {
      throw new Error(`No TokenNetwork found for token ${tokenAddress}`);
    }

    // Create contract instance and cache it
    const tokenNetwork = new ethers.Contract(networkAddress, TOKEN_NETWORK_ABI, signer);
    this.tokenNetworkCache.set(tokenAddress, tokenNetwork);

    this.logger.debug('TokenNetwork contract cached', { tokenAddress, networkAddress });

    return tokenNetwork;
  }

  /**
   * Get TokenNetwork address for a given token
   * Public method for external access to TokenNetwork addresses
   *
   * @param tokenAddress - ERC20 token address
   * @returns TokenNetwork contract address
   */
  async getTokenNetworkAddress(tokenAddress: string): Promise<string> {
    const contract = await this.getTokenNetworkContract(tokenAddress);
    return await contract.getAddress();
  }

  /**
   * Get the chain ID from the connected provider
   * Public method for Epic 31 self-describing claims
   *
   * @returns EVM chain ID as a number
   */
  async getChainId(): Promise<number> {
    const network = await this.provider.getNetwork();
    return Number(network.chainId);
  }

  /**
   * Query the on-chain ERC-20 `symbol()` for a given token address.
   *
   * Uses the read-only provider (no signer needed).
   *
   * @param tokenAddress - ERC-20 contract address
   * @returns The token symbol string (e.g. 'M2M', 'USDC')
   */
  async getTokenSymbol(tokenAddress: string): Promise<string> {
    const { ethers } = await requireOptional<typeof import('ethers')>('ethers', 'EVM settlement');
    const erc20 = new ethers.Contract(tokenAddress, ERC20_ABI, this.provider);
    const symbol: string = await erc20.symbol!();
    return symbol;
  }

  /**
   * Get the signer's Ethereum address
   *
   * @returns Ethereum address of the signer
   */
  async getSignerAddress(): Promise<string> {
    const { signer } = await this._ensureInitialized();
    return await signer.getAddress();
  }

  /**
   * Open a new payment channel with another participant
   *
   * @param participant2 - Counterparty address
   * @param tokenAddress - ERC20 token address for this channel
   * @param settlementTimeout - Challenge period duration in seconds
   * @param initialDeposit - Initial deposit amount (0 for no deposit)
   * @returns Object with channelId and txHash
   */
  async openChannel(
    participant2: string,
    tokenAddress: string,
    settlementTimeout: number,
    initialDeposit: bigint
  ): Promise<{ channelId: string; txHash: string }> {
    const tokenNetwork = await this.getTokenNetworkContract(tokenAddress);

    // Call openChannel on TokenNetwork contract
    this.logger.info('Opening payment channel', {
      participant2,
      tokenAddress,
      settlementTimeout,
      initialDeposit: initialDeposit.toString(),
    });

    const tx = await tokenNetwork.openChannel!(participant2, settlementTimeout);
    const receipt = await tx.wait();

    // Parse ChannelOpened event to extract channelId
    const channelOpenedEvent = receipt.logs
      .map((log: Log) => {
        try {
          return tokenNetwork.interface.parseLog({
            topics: log.topics as string[],
            data: log.data,
          });
        } catch {
          return null;
        }
      })
      .find((parsed: LogDescription | null) => parsed?.name === 'ChannelOpened');

    if (!channelOpenedEvent) {
      throw new Error('ChannelOpened event not found in transaction receipt');
    }

    const channelId = channelOpenedEvent.args[0] as string;
    const [participant1, participant2Addr] = [
      channelOpenedEvent.args[1] as string,
      channelOpenedEvent.args[2] as string,
    ];

    // Initialize channel state cache
    const participants: [string, string] = [participant1, participant2Addr];
    this.channelStateCache.set(channelId, {
      channelId,
      participants,
      myDeposit: 0n,
      theirDeposit: 0n,
      myNonce: 0,
      theirNonce: 0,
      myTransferred: 0n,
      theirTransferred: 0n,
      status: 'opened',
      settlementTimeout,
      openedAt: Date.now() / 1000, // Approximate timestamp
    });

    const txHash = receipt.hash;

    this.logger.info('Channel opened successfully', {
      channelId,
      participant1,
      participant2: participant2Addr,
      txHash,
    });

    // Handle initial deposit if specified
    if (initialDeposit > 0n) {
      await this.deposit(channelId, tokenAddress, initialDeposit);
    }

    return { channelId, txHash };
  }

  /**
   * Deposit additional tokens to an open channel
   *
   * @param channelId - Channel identifier
   * @param tokenAddress - ERC20 token address
   * @param amount - Amount to deposit
   */
  async deposit(channelId: string, tokenAddress: string, amount: bigint): Promise<void> {
    const { signer, ethers } = await this._ensureInitialized();
    const tokenNetwork = await this.getTokenNetworkContract(tokenAddress);
    const myAddress = await signer.getAddress();

    // Get current channel state
    const state = await this.getChannelState(channelId, tokenAddress);
    const newTotalDeposit = state.myDeposit + amount;

    this.logger.info('Depositing to channel', {
      channelId,
      amount: amount.toString(),
      newTotalDeposit: newTotalDeposit.toString(),
    });

    // Check current allowance and approve if needed
    // Use max uint256 approval to avoid repeated approvals for each deposit
    const token = new ethers.Contract(tokenAddress, ERC20_ABI, signer);
    const tokenNetworkAddress = await tokenNetwork.getAddress();
    const currentAllowance = (await token.allowance!(myAddress, tokenNetworkAddress)) as bigint;

    // Approve max uint256 if current allowance is insufficient
    // This avoids race conditions with multiple concurrent settlements
    if (currentAllowance < newTotalDeposit) {
      const maxApproval = ethers.MaxUint256;
      this.logger.info('Approving max token allowance for TokenNetwork', {
        channelId,
        currentAllowance: currentAllowance.toString(),
        requiredAmount: newTotalDeposit.toString(),
        approvalAmount: 'max uint256',
        tokenNetworkAddress,
      });

      // Retry approve transaction with fresh nonce if nonce error occurs
      let approveTx;
      let retries = 0;
      const maxRetries = 3;

      while (retries < maxRetries) {
        try {
          // Explicitly fetch fresh nonce for each attempt
          const currentNonce = await this.provider.getTransactionCount(myAddress, 'pending');
          this.logger.debug('Sending approve transaction', {
            channelId,
            nonce: currentNonce,
            attempt: retries + 1,
          });

          approveTx = await token.approve!(tokenNetworkAddress, maxApproval, {
            nonce: currentNonce,
          });
          break; // Success, exit retry loop
        } catch (error: unknown) {
          const err = error as { code?: string; message?: string };
          if (err.code === 'NONCE_EXPIRED' && retries < maxRetries - 1) {
            this.logger.warn('Nonce error on approve, retrying with fresh nonce', {
              channelId,
              attempt: retries + 1,
              error: err.message,
            });
            retries++;
            // Wait briefly before retry to allow pending transactions to settle
            await new Promise((resolve) => setTimeout(resolve, 1000));
          } else {
            // Not a nonce error or max retries reached, rethrow
            throw error;
          }
        }
      }

      if (!approveTx) {
        throw new Error('Failed to send approve transaction after retries');
      }

      await approveTx.wait();

      this.logger.debug('Token approval confirmed', {
        channelId,
        approvedAmount: 'max uint256',
        txHash: approveTx.hash,
      });
    } else {
      this.logger.debug('Sufficient token allowance already exists', {
        channelId,
        currentAllowance: currentAllowance.toString(),
        requiredAmount: newTotalDeposit.toString(),
      });
    }

    // Call setTotalDeposit with retry logic for nonce errors
    let depositTx;
    let depositRetries = 0;
    const maxDepositRetries = 3;

    while (depositRetries < maxDepositRetries) {
      try {
        const currentNonce = await this.provider.getTransactionCount(myAddress, 'pending');
        this.logger.debug('Sending setTotalDeposit transaction', {
          channelId,
          nonce: currentNonce,
          attempt: depositRetries + 1,
        });

        depositTx = await tokenNetwork.setTotalDeposit!(channelId, myAddress, newTotalDeposit, {
          nonce: currentNonce,
        });
        break;
      } catch (error: unknown) {
        const err = error as { code?: string; message?: string };
        if (err.code === 'NONCE_EXPIRED' && depositRetries < maxDepositRetries - 1) {
          this.logger.warn('Nonce error on setTotalDeposit, retrying', {
            channelId,
            attempt: depositRetries + 1,
            error: err.message,
          });
          depositRetries++;
          await new Promise((resolve) => setTimeout(resolve, 1000));
        } else {
          throw error;
        }
      }
    }

    if (!depositTx) {
      throw new Error('Failed to send setTotalDeposit transaction after retries');
    }

    await depositTx.wait();

    // Update cached state
    if (this.channelStateCache.has(channelId)) {
      const cached = this.channelStateCache.get(channelId)!;
      cached.myDeposit = newTotalDeposit;
      this.channelStateCache.set(channelId, cached);
    }

    this.logger.info('Deposit completed', {
      channelId,
      newTotalDeposit: newTotalDeposit.toString(),
    });
  }

  /**
   * Sign a balance proof using EIP-712
   *
   * @param channelId - Channel identifier
   * @param nonce - Monotonically increasing nonce
   * @param transferredAmount - Cumulative amount transferred to counterparty
   * @param lockedAmount - Amount in pending HTLCs (0 for now)
   * @param locksRoot - Merkle root of locked transfers (bytes32(0) for now)
   * @returns EIP-712 signature
   */
  async signBalanceProof(
    channelId: string,
    nonce: number,
    transferredAmount: bigint,
    lockedAmount: bigint = 0n,
    locksRoot?: string
  ): Promise<string> {
    const { ethers } = await this._ensureInitialized();
    const resolvedLocksRoot = locksRoot ?? ethers.ZeroHash;

    // Determine which TokenNetwork this channel belongs to by querying all cached networks
    let tokenNetworkAddress: string | undefined;
    for (const [, contract] of this.tokenNetworkCache) {
      try {
        const channelData = await contract.channels!(channelId);
        if (channelData.state !== 0) {
          // NonExistent = 0
          tokenNetworkAddress = await contract.getAddress();
          break;
        }
      } catch {
        // Channel doesn't exist in this network, continue
        continue;
      }
    }

    if (!tokenNetworkAddress) {
      throw new Error(`Cannot determine TokenNetwork for channel ${channelId}`);
    }

    // Get chain ID
    const network = await this.provider.getNetwork();
    const chainId = network.chainId;

    // Build EIP-712 domain and types
    const domain = getDomainSeparator(chainId, tokenNetworkAddress);
    const types = getBalanceProofTypes();

    // Build balance proof object
    const balanceProof: BalanceProof = {
      channelId,
      nonce,
      transferredAmount,
      lockedAmount,
      locksRoot: resolvedLocksRoot,
    };

    // Create EIP-712 hash
    const hash = ethers.TypedDataEncoder.hash(domain, types, balanceProof);

    // Sign the hash with KeyManager
    const signatureBuffer = await this.keyManager.sign(
      Buffer.from(hash.slice(2), 'hex'),
      this.evmKeyId
    );

    // Convert signature Buffer to hex string for blockchain submission
    const signature = '0x' + signatureBuffer.toString('hex');

    this.logger.debug('Balance proof signed', {
      channelId,
      nonce,
      transferredAmount: transferredAmount.toString(),
    });

    return signature;
  }

  /**
   * Verify a balance proof signature
   *
   * @param balanceProof - Balance proof to verify
   * @param signature - EIP-712 signature
   * @param expectedSigner - Expected signer address
   * @returns True if signature is valid
   */
  async verifyBalanceProof(
    balanceProof: BalanceProof,
    signature: string,
    expectedSigner: string
  ): Promise<boolean> {
    try {
      const { ethers } = await this._ensureInitialized();

      // Determine TokenNetwork address for this channel
      let tokenNetworkAddress: string | undefined;
      for (const [, contract] of this.tokenNetworkCache) {
        try {
          const channelData = await contract.channels!(balanceProof.channelId);
          if (channelData.state !== 0) {
            tokenNetworkAddress = await contract.getAddress();
            break;
          }
        } catch {
          continue;
        }
      }

      if (!tokenNetworkAddress) {
        this.logger.warn('Cannot determine TokenNetwork for balance proof verification', {
          channelId: balanceProof.channelId,
        });
        return false;
      }

      // Get chain ID
      const network = await this.provider.getNetwork();
      const chainId = network.chainId;

      // Build EIP-712 domain and types
      const domain = getDomainSeparator(chainId, tokenNetworkAddress);
      const types = getBalanceProofTypes();

      // Recover signer from signature
      const recoveredSigner = ethers.verifyTypedData(domain, types, balanceProof, signature);

      // Compare addresses (case-insensitive)
      const isValid = recoveredSigner.toLowerCase() === expectedSigner.toLowerCase();

      if (!isValid) {
        this.logger.warn('Balance proof verification failed', {
          balanceProof,
          expectedSigner,
          recoveredSigner,
        });
      }

      return isValid;
    } catch (error) {
      this.logger.error('Balance proof verification error', { balanceProof, error });
      return false;
    }
  }

  /**
   * Close a payment channel, starting the grace period for claims.
   * The receiver can submit claims via claimFromChannel during the grace period.
   * After the grace period, settleChannel returns remaining funds to the depositor.
   *
   * @param channelId - Channel identifier
   * @param tokenAddress - ERC20 token address
   */
  async closeChannel(channelId: string, tokenAddress: string): Promise<void> {
    const tokenNetwork = await this.getTokenNetworkContract(tokenAddress);
    const state = await this.getChannelState(channelId, tokenAddress);

    // Validate channel is opened
    if (state.status !== 'opened') {
      throw new Error(`Cannot close channel in status: ${state.status}`);
    }

    this.logger.info('Closing channel (starting grace period)', { channelId });

    // Call closeChannel on contract — starts grace period for claims
    const tx = await tokenNetwork.closeChannel!(channelId);
    const receipt = await tx.wait();

    // Update cached state
    if (this.channelStateCache.has(channelId)) {
      const cached = this.channelStateCache.get(channelId)!;
      cached.status = 'closed';
      const block = await this.provider.getBlock(receipt.blockNumber);
      cached.closedAt = block!.timestamp;
      this.channelStateCache.set(channelId, cached);
    }

    this.logger.info('Channel closed, grace period started', { channelId, txHash: receipt.hash });
  }

  /**
   * Claim transferred funds from a channel using counterparty's signed balance proof.
   * Works on both opened and closed channels (claims allowed during grace period).
   * Only the delta since last claim is transferred (prevents double-pay).
   *
   * @param channelId - Channel identifier
   * @param tokenAddress - ERC20 token address
   * @param balanceProof - Balance proof signed by the counterparty (sender)
   * @param signature - EIP-712 signature of the balance proof
   */
  async claimFromChannel(
    channelId: string,
    tokenAddress: string,
    balanceProof: BalanceProof,
    signature: string
  ): Promise<void> {
    const tokenNetwork = await this.getTokenNetworkContract(tokenAddress);
    const state = await this.getChannelState(channelId, tokenAddress);

    // Validate channel is opened or closed (claims allowed during grace period)
    if (state.status !== 'opened' && state.status !== 'closed') {
      throw new Error(`Cannot claim from channel in status: ${state.status}`);
    }

    this.logger.info('Claiming from channel', {
      channelId,
      nonce: balanceProof.nonce,
      transferredAmount: balanceProof.transferredAmount.toString(),
    });

    // Call claimFromChannel on contract
    const tx = await tokenNetwork.claimFromChannel!(channelId, balanceProof, signature);
    const receipt = await tx.wait();

    // Invalidate cached state (deposit/balance changed on-chain)
    this.channelStateCache.delete(channelId);

    this.logger.info('Claim from channel completed', { channelId, txHash: receipt.hash });
  }

  /**
   * Query the cumulative amount already claimed by a participant from a channel
   *
   * @param channelId - Channel identifier
   * @param tokenAddress - ERC20 token address
   * @param participant - Address to query claimed amount for
   * @returns Cumulative claimed amount
   */
  async getClaimedAmount(
    channelId: string,
    tokenAddress: string,
    participant: string
  ): Promise<bigint> {
    const tokenNetwork = await this.getTokenNetworkContract(tokenAddress);
    const claimed = await tokenNetwork.claimedAmounts!(channelId, participant);
    return claimed as bigint;
  }

  /**
   * Settle a closed channel after challenge period expires
   *
   * @param channelId - Channel identifier
   * @param tokenAddress - ERC20 token address
   */
  async settleChannel(channelId: string, tokenAddress: string): Promise<void> {
    const tokenNetwork = await this.getTokenNetworkContract(tokenAddress);
    const state = await this.getChannelState(channelId, tokenAddress);

    // Validate channel is closed
    if (state.status !== 'closed') {
      throw new Error(`Cannot settle channel in status: ${state.status}`);
    }

    // Validate challenge period has expired
    if (!state.closedAt) {
      throw new Error('Channel closedAt timestamp is missing');
    }

    const latestBlock = await this.provider.getBlock('latest');
    const now = latestBlock!.timestamp;
    const expiresAt = state.closedAt + state.settlementTimeout;

    if (now < expiresAt) {
      throw new ChallengeNotExpiredError(
        `Challenge period not expired. Expires at ${new Date(expiresAt * 1000).toISOString()}`,
        channelId,
        state.closedAt,
        state.settlementTimeout
      );
    }

    this.logger.info('Settling channel', { channelId });

    // Call settleChannel on contract
    const tx = await tokenNetwork.settleChannel!(channelId);
    const receipt = await tx.wait();

    // Update cached state
    if (this.channelStateCache.has(channelId)) {
      const cached = this.channelStateCache.get(channelId)!;
      cached.status = 'settled';
      this.channelStateCache.set(channelId, cached);
    }

    this.logger.info('Channel settled', { channelId, txHash: receipt.hash });
  }

  /**
   * Get channel state from blockchain or cache
   *
   * @param channelId - Channel identifier
   * @param tokenAddress - ERC20 token address (needed to determine which TokenNetwork)
   * @returns Channel state
   */
  async getChannelState(channelId: string, tokenAddress: string): Promise<ChannelState> {
    // Check cache first
    if (this.channelStateCache.has(channelId)) {
      return this.channelStateCache.get(channelId)!;
    }

    // Query on-chain state
    const { signer } = await this._ensureInitialized();
    const tokenNetwork = await this.getTokenNetworkContract(tokenAddress);
    const myAddress = await signer.getAddress();

    // Query channel info
    const channelData = await tokenNetwork.channels!(channelId);
    const [settlementTimeout, stateEnum, closedAt, openedAt, participant1, participant2] = [
      channelData.settlementTimeout as bigint,
      channelData.state as number,
      channelData.closedAt as bigint,
      channelData.openedAt as bigint,
      channelData.participant1 as string,
      channelData.participant2 as string,
    ];

    // Map state enum to status string
    const stateMap: Record<number, 'opened' | 'closed' | 'settled'> = {
      0: 'settled', // NonExistent - treat as settled
      1: 'opened', // Opened
      2: 'closed', // Closed
      3: 'settled', // Settled
    };
    const status = stateMap[stateEnum] || 'settled';

    // Query participant states
    const myParticipantData = await tokenNetwork.participants!(channelId, myAddress);
    const counterparty =
      participant1.toLowerCase() === myAddress.toLowerCase() ? participant2 : participant1;
    const theirParticipantData = await tokenNetwork.participants!(channelId, counterparty);

    // Build channel state
    const state: ChannelState = {
      channelId,
      participants: [participant1, participant2],
      myDeposit: myParticipantData.deposit as bigint,
      theirDeposit: theirParticipantData.deposit as bigint,
      myNonce: Number(myParticipantData.nonce),
      theirNonce: Number(theirParticipantData.nonce),
      myTransferred: myParticipantData.transferredAmount as bigint,
      theirTransferred: theirParticipantData.transferredAmount as bigint,
      status,
      settlementTimeout: Number(settlementTimeout),
      closedAt: closedAt > 0 ? Number(closedAt) : undefined,
      openedAt: Number(openedAt),
    };

    // Cache state
    this.channelStateCache.set(channelId, state);

    return state;
  }

  /**
   * Get channel state directly from a TokenNetwork contract address (no registry lookup)
   * Used for verifying unknown channels from self-describing claims.
   * Read-only operation -- no signer needed, only provider.
   *
   * @param channelId - Channel identifier (bytes32)
   * @param tokenNetworkAddress - TokenNetwork contract address
   * @returns Simplified channel state for verification
   */
  async getChannelStateByNetwork(
    channelId: string,
    tokenNetworkAddress: string
  ): Promise<{
    exists: boolean;
    state: number;
    participant1: string;
    participant2: string;
    settlementTimeout: number;
  }> {
    const { ethers } = await requireOptional<typeof import('ethers')>('ethers', 'EVM settlement');

    const tokenNetwork = new ethers.Contract(tokenNetworkAddress, TOKEN_NETWORK_ABI, this.provider);

    try {
      const channelData = await tokenNetwork.channels!(channelId);
      const stateEnum = Number(channelData.state);

      return {
        exists: stateEnum !== 0,
        state: stateEnum,
        participant1: channelData.participant1 as string,
        participant2: channelData.participant2 as string,
        settlementTimeout: Number(channelData.settlementTimeout),
      };
    } catch (error) {
      this.logger.error('Failed to query channel state by network', {
        channelId,
        tokenNetworkAddress,
        error,
      });
      throw error;
    }
  }

  /**
   * Verify a balance proof signature using an explicit EIP-712 domain
   * Used for verifying claims from unknown channels where the TokenNetwork
   * is not in the registry cache. Read-only -- no signer needed.
   *
   * @param balanceProof - Balance proof to verify
   * @param signature - EIP-712 signature
   * @param expectedSigner - Expected signer address
   * @param chainId - EVM chain ID for domain construction
   * @param tokenNetworkAddress - TokenNetwork contract address for domain construction
   * @returns True if signature is valid
   */
  async verifyBalanceProofWithDomain(
    balanceProof: BalanceProof,
    signature: string,
    expectedSigner: string,
    chainId: number,
    tokenNetworkAddress: string
  ): Promise<boolean> {
    try {
      const { ethers } = await requireOptional<typeof import('ethers')>('ethers', 'EVM settlement');

      const domain = getDomainSeparator(chainId, tokenNetworkAddress);
      const types = getBalanceProofTypes();

      const recoveredSigner = ethers.verifyTypedData(domain, types, balanceProof, signature);

      const isValid = recoveredSigner.toLowerCase() === expectedSigner.toLowerCase();

      if (!isValid) {
        this.logger.warn('Balance proof verification with explicit domain failed', {
          channelId: balanceProof.channelId,
          expectedSigner,
          recoveredSigner,
          chainId,
          tokenNetworkAddress,
        });
      }

      return isValid;
    } catch (error) {
      this.logger.error('Balance proof verification with explicit domain error', {
        channelId: balanceProof.channelId,
        chainId,
        tokenNetworkAddress,
        error,
      });
      return false;
    }
  }

  /**
   * Get all channel IDs for the current signer and token
   *
   * @param tokenAddress - ERC20 token address
   * @returns Array of channel IDs
   */
  async getMyChannels(tokenAddress: string): Promise<string[]> {
    const { signer } = await this._ensureInitialized();
    const tokenNetwork = await this.getTokenNetworkContract(tokenAddress);
    const myAddress = await signer.getAddress();

    // Query all ChannelOpened events
    const filter = tokenNetwork.filters.ChannelOpened!();
    const events = await tokenNetwork.queryFilter(filter);

    // Filter events where I am a participant
    const myChannels = events
      .filter((event) => {
        const eventLog = event as EventLog;
        const participant1 = eventLog.args[1] as string;
        const participant2 = eventLog.args[2] as string;
        return (
          participant1.toLowerCase() === myAddress.toLowerCase() ||
          participant2.toLowerCase() === myAddress.toLowerCase()
        );
      })
      .map((event) => {
        const eventLog = event as EventLog;
        return eventLog.args[0] as string;
      });

    return myChannels;
  }

  /**
   * Register callback for ChannelOpened events
   *
   * @param tokenAddress - ERC20 token address to listen for
   * @param callback - Callback function to invoke on event
   */
  async onChannelOpened(
    tokenAddress: string,
    callback: (event: ChannelOpenedEvent) => void
  ): Promise<void> {
    const tokenNetwork = await this.getTokenNetworkContract(tokenAddress);

    const listener = (
      channelId: string,
      participant1: string,
      participant2: string,
      settlementTimeout: bigint
    ): void => {
      const event: ChannelOpenedEvent = {
        type: 'ChannelOpened',
        channelId,
        participant1,
        participant2,
        settlementTimeout: Number(settlementTimeout),
      };

      // Update cache
      this.channelStateCache.set(channelId, {
        channelId,
        participants: [participant1, participant2],
        myDeposit: 0n,
        theirDeposit: 0n,
        myNonce: 0,
        theirNonce: 0,
        myTransferred: 0n,
        theirTransferred: 0n,
        status: 'opened',
        settlementTimeout: Number(settlementTimeout),
        openedAt: Date.now() / 1000,
      });

      callback(event);
    };

    tokenNetwork.on('ChannelOpened', listener);

    // Track listener for cleanup
    const key = `${tokenAddress}:ChannelOpened`;
    if (!this.eventListeners.has(key)) {
      this.eventListeners.set(key, []);
    }
    this.eventListeners.get(key)!.push(listener);
  }

  /**
   * Register callback for ChannelClosed events
   *
   * @param tokenAddress - ERC20 token address to listen for
   * @param callback - Callback function to invoke on event
   */
  async onChannelClosed(
    tokenAddress: string,
    callback: (event: ChannelClosedEvent) => void
  ): Promise<void> {
    const tokenNetwork = await this.getTokenNetworkContract(tokenAddress);

    const listener = (
      channelId: string,
      closingParticipant: string,
      nonce: bigint,
      balanceHash: string
    ): void => {
      const event: ChannelClosedEvent = {
        type: 'ChannelClosed',
        channelId,
        closingParticipant,
        nonce: Number(nonce),
        balanceHash,
      };

      // Update cache
      if (this.channelStateCache.has(channelId)) {
        const cached = this.channelStateCache.get(channelId)!;
        cached.status = 'closed';
        cached.closedAt = Date.now() / 1000;
        this.channelStateCache.set(channelId, cached);
      }

      callback(event);
    };

    tokenNetwork.on('ChannelClosed', listener);

    // Track listener for cleanup
    const key = `${tokenAddress}:ChannelClosed`;
    if (!this.eventListeners.has(key)) {
      this.eventListeners.set(key, []);
    }
    this.eventListeners.get(key)!.push(listener);
  }

  /**
   * Register callback for ChannelSettled events
   *
   * @param tokenAddress - ERC20 token address to listen for
   * @param callback - Callback function to invoke on event
   */
  async onChannelSettled(
    tokenAddress: string,
    callback: (event: ChannelSettledEvent) => void
  ): Promise<void> {
    const tokenNetwork = await this.getTokenNetworkContract(tokenAddress);

    const listener = (
      channelId: string,
      participant1Amount: bigint,
      participant2Amount: bigint
    ): void => {
      const event: ChannelSettledEvent = {
        type: 'ChannelSettled',
        channelId,
        participant1Amount,
        participant2Amount,
      };

      // Update cache
      if (this.channelStateCache.has(channelId)) {
        const cached = this.channelStateCache.get(channelId)!;
        cached.status = 'settled';
        this.channelStateCache.set(channelId, cached);
      }

      callback(event);
    };

    tokenNetwork.on('ChannelSettled', listener);

    // Track listener for cleanup
    const key = `${tokenAddress}:ChannelSettled`;
    if (!this.eventListeners.has(key)) {
      this.eventListeners.set(key, []);
    }
    this.eventListeners.get(key)!.push(listener);
  }

  /**
   * Register callback for ChannelCooperativeSettled events
   *
   * @param tokenAddress - ERC20 token address to listen for
   * @param callback - Callback function to invoke on event
   */
  async onChannelCooperativeSettled(
    tokenAddress: string,
    callback: (event: ChannelCooperativeSettledEvent) => void
  ): Promise<void> {
    const tokenNetwork = await this.getTokenNetworkContract(tokenAddress);

    const listener = (
      channelId: string,
      participant1Amount: bigint,
      participant2Amount: bigint
    ): void => {
      const event: ChannelCooperativeSettledEvent = {
        type: 'ChannelCooperativeSettled',
        channelId,
        participant1Amount,
        participant2Amount,
      };

      // Update cache
      if (this.channelStateCache.has(channelId)) {
        const cached = this.channelStateCache.get(channelId)!;
        cached.status = 'settled';
        this.channelStateCache.set(channelId, cached);
      }

      callback(event);
    };

    tokenNetwork.on('ChannelCooperativeSettled', listener);

    // Track listener for cleanup
    const key = `${tokenAddress}:ChannelCooperativeSettled`;
    if (!this.eventListeners.has(key)) {
      this.eventListeners.set(key, []);
    }
    this.eventListeners.get(key)!.push(listener);
  }

  /**
   * Remove all event listeners
   * Should be called when SDK is no longer needed to prevent memory leaks
   */
  removeAllListeners(): void {
    for (const [, contract] of this.tokenNetworkCache) {
      contract.removeAllListeners();
    }
    this.eventListeners.clear();
    this.logger.debug('All event listeners removed');
  }
}
