import type { Wallet, Provider } from 'ethers';
import pino from 'pino';
import { requireOptional } from '../utils/optional-require';

const logger = pino({ name: 'treasury-wallet' });

/**
 * ERC20 ABI for transfer function only
 */
const ERC20_ABI = [
  'function transfer(address to, uint256 amount) returns (bool)',
  'function balanceOf(address owner) view returns (uint256)',
];

/**
 * Transaction result interface
 */
export interface Transaction {
  hash: string;
  to: string;
  value?: string;
}

/**
 * TreasuryWallet manages the platform's treasury for funding agent wallets.
 *
 * Handles:
 * - ETH transfers for EVM gas
 * - ERC20 token transfers for platform tokens
 *
 * Security: Private keys loaded from environment variables only.
 * NEVER stores or logs private keys.
 */
export class TreasuryWallet {
  private evmWallet: Wallet | null = null;
  private evmProvider: Provider;
  public evmAddress: string = '';
  private noncePromise: Promise<number> | null = null;
  private evmPrivateKey: string;

  /**
   * Creates a new TreasuryWallet instance
   *
   * @param evmPrivateKey - EVM private key (hex string with 0x prefix)
   * @param evmProvider - Ethers provider for EVM blockchain
   */
  constructor(evmPrivateKey: string, evmProvider: Provider) {
    // Validate private key is present
    if (!evmPrivateKey) {
      throw new Error('Treasury private key is required');
    }

    // Store private key for lazy wallet initialization
    this.evmPrivateKey = evmPrivateKey;
    this.evmProvider = evmProvider;

    logger.info('Treasury wallet config stored (wallet initialization deferred)');
  }

  /**
   * Lazily initialize the EVM wallet via dynamic import of ethers.
   */
  private async ensureEvmInitialized(): Promise<Wallet> {
    if (this.evmWallet) return this.evmWallet;

    try {
      const { ethers } = await requireOptional<typeof import('ethers')>('ethers', 'EVM settlement');
      this.evmWallet = new ethers.Wallet(this.evmPrivateKey, this.evmProvider);
      this.evmAddress = this.evmWallet.address;

      logger.info('EVM wallet initialized', { evmAddress: this.evmAddress });
      return this.evmWallet;
    } catch (error) {
      if (error instanceof Error && error.message.includes('is required for')) {
        throw error;
      }
      logger.error('Failed to initialize EVM wallet', {
        error: error instanceof Error ? error.message : String(error),
      });
      throw new Error('Failed to initialize treasury EVM wallet');
    }
  }

  /**
   * Gets the next nonce for EVM transactions, serializing requests to prevent nonce conflicts
   */
  private getNextNonce(): Promise<number> {
    // Chain the new nonce request after any pending one
    this.noncePromise = this.noncePromise
      ? this.noncePromise.then(async (prevNonce) => prevNonce + 1)
      : this.evmProvider.getTransactionCount(this.evmAddress, 'pending');

    return this.noncePromise;
  }

  /**
   * Sends ETH from treasury to recipient address
   *
   * @param to - Recipient EVM address
   * @param amount - Amount in wei (bigint)
   * @returns Transaction object with hash
   */
  async sendETH(to: string, amount: bigint): Promise<Transaction> {
    try {
      const { ethers } = await requireOptional<typeof import('ethers')>('ethers', 'EVM settlement');
      const evmWallet = await this.ensureEvmInitialized();

      // Validate recipient address
      if (!ethers.isAddress(to)) {
        throw new Error(`Invalid EVM address: ${to}`);
      }

      // Get next nonce - this serializes concurrent requests
      const nonce = await this.getNextNonce();

      // Get current fee data for gas pricing
      const feeData = await this.evmProvider.getFeeData();

      // Create transaction
      const tx = await evmWallet.sendTransaction({
        to,
        value: amount,
        nonce,
        gasLimit: 21000, // Standard ETH transfer gas limit
        maxFeePerGas: feeData.maxFeePerGas ?? undefined,
        maxPriorityFeePerGas: feeData.maxPriorityFeePerGas ?? undefined,
      });

      logger.info('ETH sent', {
        to,
        amount: amount.toString(),
        txHash: tx.hash,
        nonce,
      });

      return {
        hash: tx.hash,
        to: tx.to ?? to,
        value: amount.toString(),
      };
    } catch (error) {
      logger.error('Failed to send ETH', {
        to,
        amount: amount.toString(),
        error: error instanceof Error ? error.message : String(error),
      });
      // Reset nonce promise on error to resync with chain on next request
      this.noncePromise = null;
      throw error;
    }
  }

  /**
   * Sends ERC20 tokens from treasury to recipient address
   *
   * @param to - Recipient EVM address
   * @param tokenAddress - ERC20 token contract address
   * @param amount - Amount in token's smallest unit (bigint)
   * @returns Transaction object with hash
   */
  async sendERC20(to: string, tokenAddress: string, amount: bigint): Promise<Transaction> {
    try {
      const { ethers } = await requireOptional<typeof import('ethers')>('ethers', 'EVM settlement');
      const evmWallet = await this.ensureEvmInitialized();

      // Validate addresses
      if (!ethers.isAddress(to)) {
        throw new Error(`Invalid recipient address: ${to}`);
      }
      if (!ethers.isAddress(tokenAddress)) {
        throw new Error(`Invalid token address: ${tokenAddress}`);
      }

      // Create ERC20 contract instance
      const tokenContract = new ethers.Contract(tokenAddress, ERC20_ABI, evmWallet);

      // Send tokens
      const tx = await tokenContract.transfer!(to, amount);

      logger.info('ERC20 sent', {
        to,
        tokenAddress,
        amount: amount.toString(),
        txHash: tx.hash,
      });

      return {
        hash: tx.hash,
        to,
      };
    } catch (error) {
      logger.error('Failed to send ERC20', {
        to,
        tokenAddress,
        amount: amount.toString(),
        error: error instanceof Error ? error.message : String(error),
      });
      throw error;
    }
  }

  /**
   * Gets current balance of treasury wallet
   *
   * @param token - Token identifier ('ETH' or '0xTokenAddress')
   * @returns Balance as bigint
   */
  async getBalance(token: string): Promise<bigint> {
    try {
      const { ethers } = await requireOptional<typeof import('ethers')>('ethers', 'EVM settlement');
      await this.ensureEvmInitialized();

      if (token === 'ETH' || token.toLowerCase() === 'eth') {
        // Get ETH balance
        const balance = await this.evmProvider.getBalance(this.evmAddress);
        return balance;
      } else {
        // Get ERC20 balance
        if (!ethers.isAddress(token)) {
          throw new Error(`Invalid token address: ${token}`);
        }
        const tokenContract = new ethers.Contract(token, ERC20_ABI, this.evmProvider);
        const balance = await tokenContract.balanceOf!(this.evmAddress);
        return balance;
      }
    } catch (error) {
      logger.error('Failed to get balance', {
        token,
        error: error instanceof Error ? error.message : String(error),
      });
      throw error;
    }
  }
}
