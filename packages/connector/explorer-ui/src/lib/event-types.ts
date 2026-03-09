/**
 * Telemetry event types for Explorer UI
 *
 * Mirrors key types from @crosstown/shared for frontend use
 */

/**
 * Complete enumeration of all telemetry event types.
 * Mirrors TelemetryEventType enum from @crosstown/shared.
 */
export type TelemetryEventType =
  // Node lifecycle events
  | 'NODE_STATUS'
  // Packet flow events
  | 'PACKET_RECEIVED'
  | 'PACKET_FORWARDED'
  | 'PACKET_FULFILLED'
  | 'PACKET_REJECTED'
  // Account and settlement events
  | 'ACCOUNT_BALANCE'
  | 'SETTLEMENT_TRIGGERED'
  | 'SETTLEMENT_COMPLETED'
  // Agent wallet events
  | 'AGENT_BALANCE_CHANGED'
  | 'AGENT_WALLET_FUNDED'
  | 'AGENT_WALLET_STATE_CHANGED'
  | 'FUNDING_RATE_LIMIT_EXCEEDED'
  | 'FUNDING_TRANSACTION_CONFIRMED'
  | 'FUNDING_TRANSACTION_FAILED'
  // EVM payment channel events
  | 'PAYMENT_CHANNEL_OPENED'
  | 'PAYMENT_CHANNEL_BALANCE_UPDATE'
  | 'PAYMENT_CHANNEL_SETTLED'
  // Agent channel events
  | 'AGENT_CHANNEL_OPENED'
  | 'AGENT_CHANNEL_PAYMENT_SENT'
  | 'AGENT_CHANNEL_BALANCE_UPDATE'
  | 'AGENT_CHANNEL_CLOSED'
  // Claim exchange events (Epic 17)
  | 'CLAIM_SENT'
  | 'CLAIM_RECEIVED'
  | 'CLAIM_REDEEMED'
  // Security events
  | 'WALLET_BALANCE_MISMATCH'
  | 'SUSPICIOUS_ACTIVITY_DETECTED'
  | 'RATE_LIMIT_EXCEEDED';

/**
 * Base telemetry event interface
 */
export interface TelemetryEvent {
  type: TelemetryEventType;
  nodeId?: string;
  timestamp: string | number;
  peerId?: string;
  [key: string]: unknown;
}

/**
 * Stored event from EventStore API
 */
export interface StoredEvent {
  id: number;
  event_type: string;
  timestamp: number;
  node_id: string;
  direction: string | null;
  peer_id: string | null;
  packet_id: string | null;
  amount: string | null;
  destination: string | null;
  packet_type: string | null;
  from_address: string | null;
  to_address: string | null;
  payload: TelemetryEvent;
}

/**
 * ILP Packet types for display
 */
export type IlpPacketType = 'prepare' | 'fulfill' | 'reject';

/**
 * Packet type color mapping for ILP packet types
 * NOC aesthetic: prepare (cyan), fulfill (emerald), reject (rose)
 */
export const PACKET_TYPE_COLORS: Record<string, string> = {
  prepare: 'bg-cyan-500',
  fulfill: 'bg-emerald-500',
  reject: 'bg-rose-500',
};

/**
 * Get ILP packet type display name from event
 * Returns 'prepare', 'fulfill', or 'reject' for ILP packet events
 */
export function getIlpPacketType(event: TelemetryEvent | StoredEvent): IlpPacketType | null {
  // Check StoredEvent with packet_type field (from database)
  if ('packet_type' in event && typeof event.packet_type === 'string' && event.packet_type) {
    const type = event.packet_type.toLowerCase();
    if (type === 'prepare' || type === 'fulfill' || type === 'reject') {
      return type as IlpPacketType;
    }
  }

  // Check TelemetryEvent with packetType field (from live events)
  if ('packetType' in event && typeof event.packetType === 'string') {
    const type = event.packetType.toLowerCase();
    if (type === 'prepare' || type === 'fulfill' || type === 'reject') {
      return type as IlpPacketType;
    }
  }

  // Determine packet type from event type
  const eventType = 'type' in event ? event.type : 'event_type' in event ? event.event_type : null;

  // PACKET_RECEIVED and PACKET_FORWARDED are always 'prepare' packets
  if (eventType === 'PACKET_RECEIVED' || eventType === 'PACKET_FORWARDED') {
    return 'prepare';
  }

  // PACKET_FULFILLED events are 'fulfill' packets
  if (eventType === 'PACKET_FULFILLED') {
    return 'fulfill';
  }

  // PACKET_REJECTED events are 'reject' packets
  if (eventType === 'PACKET_REJECTED') {
    return 'reject';
  }

  return null;
}

/**
 * Check if event is an ILP packet event (prepare/fulfill/reject)
 */
export function isIlpPacketEvent(event: TelemetryEvent | StoredEvent): boolean {
  return getIlpPacketType(event) !== null;
}

/**
 * ILP Packet event types for filtering (Story 18.3 AC 5)
 * Used by FilterBar "ILP Packets" quick filter preset
 */
export const ILP_PACKET_EVENT_TYPES = [
  'PACKET_RECEIVED',
  'PACKET_FORWARDED',
  'PACKET_FULFILLED',
  'PACKET_REJECTED',
  'AGENT_CHANNEL_PAYMENT_SENT',
] as const;

/**
 * Response from GET /api/events
 */
export interface EventsResponse {
  events: StoredEvent[];
  total: number;
  limit: number;
  offset: number;
}

/**
 * Response from GET /api/health
 */
export interface HealthResponse {
  status: 'healthy' | 'degraded';
  nodeId: string;
  uptime: number;
  explorer: {
    eventCount: number;
    databaseSizeBytes: number;
    wsConnections: number;
  };
  timestamp: string;
}

/**
 * Event type color mapping for badges.
 * Uses Tailwind CSS color classes (included in default Tailwind palette).
 */
export const EVENT_TYPE_COLORS: Record<string, string> = {
  // Node lifecycle - gray (neutral)
  NODE_STATUS: 'bg-gray-500',
  // Packet flow - blue shades for prepare, green/red for fulfill/reject
  PACKET_RECEIVED: 'bg-blue-400',
  PACKET_FORWARDED: 'bg-blue-600',
  PACKET_FULFILLED: 'bg-green-500',
  PACKET_REJECTED: 'bg-red-500',
  // Account and settlement - green/yellow
  ACCOUNT_BALANCE: 'bg-blue-500',
  SETTLEMENT_TRIGGERED: 'bg-yellow-500',
  SETTLEMENT_COMPLETED: 'bg-green-500',
  // Agent wallet - purple/indigo
  AGENT_BALANCE_CHANGED: 'bg-purple-500',
  AGENT_WALLET_FUNDED: 'bg-indigo-500',
  AGENT_WALLET_STATE_CHANGED: 'bg-cyan-500',
  FUNDING_RATE_LIMIT_EXCEEDED: 'bg-amber-500',
  FUNDING_TRANSACTION_CONFIRMED: 'bg-lime-500',
  FUNDING_TRANSACTION_FAILED: 'bg-rose-500',
  // EVM payment channels - emerald/teal
  PAYMENT_CHANNEL_OPENED: 'bg-emerald-500',
  PAYMENT_CHANNEL_BALANCE_UPDATE: 'bg-teal-500',
  PAYMENT_CHANNEL_SETTLED: 'bg-green-600',
  // Agent channels - violet
  AGENT_CHANNEL_OPENED: 'bg-violet-500',
  AGENT_CHANNEL_PAYMENT_SENT: 'bg-violet-400',
  AGENT_CHANNEL_BALANCE_UPDATE: 'bg-violet-500',
  AGENT_CHANNEL_CLOSED: 'bg-violet-600',
  // Claim exchange events - pink/fuchsia theme (Epic 17)
  CLAIM_SENT: 'bg-fuchsia-500',
  CLAIM_RECEIVED: 'bg-fuchsia-400',
  CLAIM_REDEEMED: 'bg-fuchsia-600',
  // Security events - red shades
  WALLET_BALANCE_MISMATCH: 'bg-red-500',
  SUSPICIOUS_ACTIVITY_DETECTED: 'bg-red-600',
  RATE_LIMIT_EXCEEDED: 'bg-red-400',
};

/**
 * Format relative timestamp (e.g., "2s ago")
 */
export function formatRelativeTime(timestamp: number): string {
  const now = Date.now();
  const diff = now - timestamp;

  if (diff < 1000) return 'just now';
  if (diff < 60000) return `${Math.floor(diff / 1000)}s ago`;
  if (diff < 3600000) return `${Math.floor(diff / 60000)}m ago`;
  if (diff < 86400000) return `${Math.floor(diff / 3600000)}h ago`;
  return new Date(timestamp).toLocaleString();
}

// ============================================================================
// Story 14.6: Account and Settlement Types
// ============================================================================

/**
 * Settlement state enumeration
 * Mirrors SettlementState from @crosstown/shared
 */
export type SettlementState = 'IDLE' | 'SETTLEMENT_PENDING' | 'SETTLEMENT_IN_PROGRESS';

/**
 * Balance history entry for tracking changes over time
 */
export interface BalanceHistoryEntry {
  timestamp: number;
  balance: bigint;
}

/**
 * Account state for frontend state management (Story 14.6)
 * Used by useAccountBalances hook to track peer account state
 */
export interface AccountState {
  peerId: string;
  tokenId: string;
  debitBalance: bigint;
  creditBalance: bigint;
  netBalance: bigint;
  creditLimit?: bigint;
  settlementThreshold?: bigint;
  settlementState: SettlementState;
  balanceHistory: BalanceHistoryEntry[];
  hasActiveChannel?: boolean;
  channelType?: 'evm';
  lastUpdated: number;
}

/**
 * Channel state for frontend (Story 14.6)
 * Mirrors DashboardChannelState from @crosstown/shared
 */
export interface ChannelState {
  channelId: string;
  nodeId: string;
  peerId: string;
  participants: [string, string];
  tokenAddress: string;
  tokenSymbol: string;
  settlementTimeout: number;
  deposits: Record<string, string>;
  myNonce: number;
  theirNonce: number;
  myTransferred: string;
  theirTransferred: string;
  status: 'opening' | 'active' | 'closing' | 'settling' | 'settled';
  openedAt: string;
  settledAt?: string;
  lastActivityAt: string;
  // Chain-specific fields
  settlementMethod?: 'evm';
}

/**
 * Settlement event types for filtering (Story 14.6)
 * Used by FilterBar "Settlement" quick filter preset
 */
export const SETTLEMENT_EVENT_TYPES = [
  'ACCOUNT_BALANCE',
  'SETTLEMENT_TRIGGERED',
  'SETTLEMENT_COMPLETED',
  'PAYMENT_CHANNEL_OPENED',
  'PAYMENT_CHANNEL_BALANCE_UPDATE',
  'PAYMENT_CHANNEL_SETTLED',
  'AGENT_CHANNEL_OPENED',
  'AGENT_CHANNEL_BALANCE_UPDATE',
  'AGENT_CHANNEL_CLOSED',
  'CLAIM_SENT',
  'CLAIM_RECEIVED',
  'CLAIM_REDEEMED',
] as const;

/**
 * Check if an event type is settlement-related (Story 14.6)
 */
export function isSettlementEvent(type: TelemetryEventType): boolean {
  return SETTLEMENT_EVENT_TYPES.includes(type as (typeof SETTLEMENT_EVENT_TYPES)[number]);
}

// ============================================================================
// On-Chain Wallet Balance Types
// ============================================================================

/**
 * EVM payment channel from /api/balances
 */
export interface WalletEvmChannel {
  channelId: string;
  peerAddress: string;
  deposit: string;
  transferredAmount: string;
  status: string;
}

/**
 * Response from GET /api/balances
 */
export interface WalletBalances {
  agentId: string;
  evmAddress: string;
  ethBalance: string | null;
  agentTokenBalance: string | null;
  evmChannels: WalletEvmChannel[];
}

// ============================================================================
// Story 17.6: Claim Exchange Event Helpers
// ============================================================================

/**
 * Blockchain type for claim events
 */
export type ClaimBlockchain = 'evm';

/**
 * Extract blockchain type from claim event
 */
export function getClaimBlockchain(event: TelemetryEvent): ClaimBlockchain | null {
  if (typeof event.blockchain === 'string') {
    const blockchain = event.blockchain.toLowerCase();
    if (blockchain === 'evm') {
      return blockchain as ClaimBlockchain;
    }
  }
  return null;
}

/**
 * Extract amount from claim event
 */
export function getClaimAmount(event: TelemetryEvent): string | null {
  return typeof event.amount === 'string' ? event.amount : null;
}

/**
 * Extract success status from claim event (CLAIM_SENT, CLAIM_REDEEMED)
 */
export function getClaimSuccess(event: TelemetryEvent): boolean | null {
  return typeof event.success === 'boolean' ? event.success : null;
}

/**
 * Extract verified status from claim event (CLAIM_RECEIVED)
 */
export function getClaimVerified(event: TelemetryEvent): boolean | null {
  return typeof event.verified === 'boolean' ? event.verified : null;
}

/**
 * Extract messageId from claim event (for correlation)
 */
export function getClaimMessageId(event: TelemetryEvent): string | null {
  return typeof event.messageId === 'string' ? event.messageId : null;
}

/**
 * Extract channelId from claim event (available in CLAIM_RECEIVED and CLAIM_REDEEMED)
 */
export function getClaimChannelId(event: TelemetryEvent): string | null {
  return typeof event.channelId === 'string' ? event.channelId : null;
}

/**
 * Format claim amount based on blockchain
 * - EVM: wei
 */
export function formatClaimAmount(amount: string): string {
  const value = BigInt(amount);
  // Convert wei to ETH (1 ETH = 10^18 wei)
  return `${(Number(value) / 1e18).toFixed(6)} ETH`;
}

/**
 * Get blockchain badge color
 */
export function getBlockchainBadgeColor(): string {
  return 'bg-blue-100 text-blue-800 border-blue-300';
}

// ============================================================================
// Block Explorer Link Utilities
// ============================================================================

/**
 * Base Sepolia block explorer URL
 */
export const BASE_SEPOLIA_EXPLORER = 'https://sepolia.basescan.org';

/**
 * Check if a string is a valid Ethereum address (0x followed by 40 hex chars)
 */
export function isEthereumAddress(value: string): boolean {
  return /^0x[a-fA-F0-9]{40}$/.test(value);
}

/**
 * Check if a string is a valid transaction hash (0x followed by 64 hex chars)
 */
export function isTransactionHash(value: string): boolean {
  return /^0x[a-fA-F0-9]{64}$/.test(value);
}

/**
 * Get block explorer URL for an Ethereum address
 */
export function getAddressExplorerUrl(address: string): string {
  return `${BASE_SEPOLIA_EXPLORER}/address/${address}`;
}

/**
 * Get block explorer URL for a transaction hash
 */
export function getTransactionExplorerUrl(txHash: string): string {
  return `${BASE_SEPOLIA_EXPLORER}/tx/${txHash}`;
}

/**
 * Truncate an Ethereum address or hash for display
 */
export function truncateHash(value: string, startChars = 6, endChars = 4): string {
  if (value.length <= startChars + endChars + 2) return value;
  return `${value.slice(0, startChars)}...${value.slice(-endChars)}`;
}
