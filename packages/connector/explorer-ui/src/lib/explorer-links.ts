/**
 * Explorer Links Utility
 *
 * Centralized utility for building blockchain explorer URLs and detecting address types.
 * Supports EVM (Base) blockchain explorers.
 */

/**
 * Supported blockchain address types
 */
export type AddressType = 'evm' | 'unknown';

/**
 * Resource types for explorer URLs
 */
export type ResourceType = 'address' | 'tx';

/**
 * Explorer configuration for testnet and mainnet URLs
 */
export interface ExplorerConfig {
  evm: {
    testnet: string;
    mainnet: string;
  };
}

/**
 * Explorer base URLs for all supported blockchains
 */
export const EXPLORER_CONFIG: ExplorerConfig = {
  evm: {
    testnet: 'https://sepolia.basescan.org',
    mainnet: 'https://etherscan.io'
  }
};

/**
 * Detect the blockchain type based on address format
 *
 * @param address - The address string to analyze
 * @returns The detected blockchain type or 'unknown'
 *
 * @example
 * detectAddressType('0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb')
 * // returns 'evm'
 */
export function detectAddressType(address: string): AddressType {
  if (!address || typeof address !== 'string') {
    return 'unknown';
  }

  // Normalize input (trim whitespace)
  const normalized = address.trim();

  // EVM addresses: 0x + 40 hex chars = 42 total
  if (/^0x[0-9a-fA-F]{40}$/.test(normalized)) {
    return 'evm';
  }

  return 'unknown';
}

/**
 * Generate a blockchain explorer URL for an address or transaction hash
 *
 * @param value - The address or transaction hash
 * @param type - The resource type ('address' or 'tx')
 * @param chain - Optional blockchain type (auto-detected if not provided)
 * @param network - Network to use ('testnet' or 'mainnet')
 * @returns The explorer URL or null if detection fails
 *
 * @example
 * getExplorerUrl('0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb', 'address')
 * // returns 'https://sepolia.basescan.org/address/0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb'
 */
export function getExplorerUrl(
  value: string,
  type: ResourceType = 'address',
  chain?: AddressType,
  network: 'testnet' | 'mainnet' = 'testnet'
): string | null {
  if (!value || typeof value !== 'string') {
    return null;
  }

  // Auto-detect chain if not provided
  const detectedChain = chain ?? detectAddressType(value);

  if (detectedChain === 'unknown') {
    return null;
  }

  const baseUrl = EXPLORER_CONFIG[detectedChain][network];

  return type === 'tx'
    ? `${baseUrl}/tx/${value}`
    : `${baseUrl}/address/${value}`;
}
