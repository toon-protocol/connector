import { Logger } from 'pino';
import type { Wallet } from 'ethers';
import { KeyManagerBackend } from '../key-manager';
import { requireOptional } from '../../utils/optional-require';

/**
 * EnvironmentVariableBackend implements KeyManagerBackend using private keys from environment variables
 * For development and testing only - not suitable for production use
 */
export class EnvironmentVariableBackend implements KeyManagerBackend {
  private evmWallet?: Wallet;
  private evmPrivateKey?: string;
  private logger: Logger;

  /**
   * @param logger - Pino logger instance
   * @param options - Optional direct key injection. When `evmPrivateKey` is provided,
   *   it bypasses `process.env.EVM_PRIVATE_KEY` lookup, enabling multi-node isolation
   *   without env var mutation.
   */
  constructor(logger: Logger, options?: { evmPrivateKey?: string }) {
    this.logger = logger.child({ component: 'EnvironmentVariableBackend' });

    // Store EVM private key for lazy wallet initialization (deferred until first use)
    // Direct injection takes precedence over environment variable
    const evmPrivateKey = options?.evmPrivateKey ?? process.env.EVM_PRIVATE_KEY;
    if (evmPrivateKey) {
      this.evmPrivateKey = evmPrivateKey;
      this.logger.info('EVM private key found in environment (wallet initialization deferred)');
    }

    if (!this.evmPrivateKey) {
      this.logger.warn('No EVM private key loaded from environment (EVM_PRIVATE_KEY)');
    }
  }

  /**
   * Lazily initialize EVM wallet on first use (avoids top-level ethers import)
   */
  private async _ensureEvmWallet(): Promise<Wallet> {
    if (this.evmWallet) {
      return this.evmWallet;
    }
    if (!this.evmPrivateKey) {
      throw new Error('EVM wallet not initialized. Set EVM_PRIVATE_KEY environment variable.');
    }
    try {
      const { Wallet } = await requireOptional<typeof import('ethers')>('ethers', 'EVM settlement');
      this.evmWallet = new Wallet(this.evmPrivateKey);
      this.logger.info({ address: this.evmWallet.address }, 'EVM wallet loaded from environment');
      return this.evmWallet;
    } catch (error) {
      if (error instanceof Error && error.message.includes('is required for')) {
        throw error; // Re-throw requireOptional errors as-is
      }
      this.logger.error({ error }, 'Failed to load EVM private key');
      throw new Error('Invalid EVM_PRIVATE_KEY in environment');
    }
  }

  /**
   * Signs a message using the EVM wallet
   * @param message - Message to sign
   * @param keyId - Key identifier
   * @returns Signature buffer
   */
  async sign(message: Buffer, _keyId: string): Promise<Buffer> {
    const evmWallet = await this._ensureEvmWallet();

    // Sign raw message hash using signingKey.sign() (NOT signMessage which adds EIP-191 prefix)
    // This is used for signing transaction hashes where we need raw ECDSA signature
    const signature = evmWallet.signingKey.sign(message);
    // Return concatenated r || s || v (65 bytes)
    return Buffer.from(signature.serialized.slice(2), 'hex'); // Remove '0x' prefix
  }

  /**
   * Retrieves public key derived from private key
   * @param keyId - Key identifier
   * @returns Public key buffer
   */
  async getPublicKey(_keyId: string): Promise<Buffer> {
    const evmWallet = await this._ensureEvmWallet();

    // Get public key from wallet (compressed secp256k1 format)
    const publicKey = evmWallet.signingKey.publicKey;
    return Buffer.from(publicKey.slice(2), 'hex'); // Remove '0x' prefix
  }

  /**
   * Key rotation not supported for environment variable backend
   * Manual rotation required (update environment variables and restart)
   * @param keyId - Key identifier
   * @throws Error indicating manual rotation required
   */
  async rotateKey(_keyId: string): Promise<string> {
    throw new Error(
      'Manual rotation required for environment backend. Update EVM_PRIVATE_KEY and restart the connector.'
    );
  }
}
