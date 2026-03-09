/**
 * Payment Channel Telemetry Event Type Definitions
 *
 * This module provides TypeScript type definitions for payment channel telemetry events
 * emitted by the ChannelManager (Story 8.9) to the dashboard for real-time visualization
 * of payment channel state and activity.
 *
 * Events support payment channel lifecycle monitoring, balance proof updates, and
 * settlement tracking for dashboard visualization (Story 8.10).
 *
 * @packageDocumentation
 */

/**
 * Payment Channel Opened Telemetry Event
 *
 * Emitted when ChannelManager opens a new payment channel.
 * Indicates channel creation on-chain with initial deposits.
 *
 * **BigInt Serialization:** All deposit fields are strings (bigint values serialized as
 * strings for JSON compatibility). Use `BigInt(value)` to convert back to bigint.
 *
 * **Emission Points:**
 * - ChannelManager.openChannelForPeer() after PaymentChannelSDK.openChannel() completes
 *
 * **Dashboard Usage:**
 * - NetworkGraph displays channel badge on peer edge
 * - PaymentChannelsPanel adds channel to active channels list
 * - TimelineView shows channel opened event
 *
 * @example
 * ```typescript
 * const event: PaymentChannelOpenedEvent = {
 *   type: 'PAYMENT_CHANNEL_OPENED',
 *   timestamp: '2026-01-09T12:00:00.000Z',
 *   nodeId: 'connector-a',
 *   channelId: '0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef',
 *   participants: ['0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb', '0x8626f6940E2eb28930eFb4CeF49B2d1F2C9C1199'],
 *   peerId: 'connector-b',
 *   tokenAddress: '0x5FbDB2315678afecb367f032d93F642f64180aa3',
 *   tokenSymbol: 'USDC',
 *   settlementTimeout: 86400,
 *   initialDeposits: {
 *     '0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb': '1000000000000000000',
 *     '0x8626f6940E2eb28930eFb4CeF49B2d1F2C9C1199': '0'
 *   }
 * };
 * ```
 */
export interface PaymentChannelOpenedEvent {
  /** Event type discriminator */
  type: 'PAYMENT_CHANNEL_OPENED';
  /** Event timestamp (ISO 8601 format) */
  timestamp: string;
  /** Connector node ID emitting event */
  nodeId: string;
  /** bytes32 channel identifier */
  channelId: string;
  /** Participant addresses [myAddress, peerAddress] */
  participants: [string, string];
  /** Peer connector ID (e.g., "connector-b") */
  peerId: string;
  /** ERC20 token contract address */
  tokenAddress: string;
  /** Human-readable token symbol (e.g., "USDC") */
  tokenSymbol: string;
  /** Challenge period duration (seconds) */
  settlementTimeout: number;
  /** Initial deposits by participant address, bigint as string */
  initialDeposits: {
    [participant: string]: string;
  };
}

/**
 * Payment Channel Balance Update Telemetry Event
 *
 * Emitted when off-chain balance proofs update channel state.
 * Indicates balance transfer between participants without on-chain transaction.
 *
 * **BigInt Serialization:** All transferred amount fields are strings (bigint serialized for JSON).
 *
 * **Emission Points:**
 * - ChannelManager after receiving CHANNEL_ACTIVITY event from SettlementExecutor
 * - ChannelManager after submitting balance update via PaymentChannelSDK
 *
 * **Dashboard Usage:**
 * - NetworkGraph updates channel badge with latest balances
 * - PaymentChannelsPanel updates balance display in real-time
 * - TimelineView shows balance update events
 *
 * @example
 * ```typescript
 * const event: PaymentChannelBalanceUpdateEvent = {
 *   type: 'PAYMENT_CHANNEL_BALANCE_UPDATE',
 *   timestamp: '2026-01-09T12:01:00.000Z',
 *   nodeId: 'connector-a',
 *   channelId: '0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef',
 *   myNonce: 5,
 *   theirNonce: 3,
 *   myTransferred: '5000000000000000000',
 *   theirTransferred: '2000000000000000000'
 * };
 * ```
 */
export interface PaymentChannelBalanceUpdateEvent {
  /** Event type discriminator */
  type: 'PAYMENT_CHANNEL_BALANCE_UPDATE';
  /** Event timestamp (ISO 8601 format) */
  timestamp: string;
  /** Connector node ID emitting event */
  nodeId: string;
  /** bytes32 channel identifier */
  channelId: string;
  /** My balance proof nonce (monotonic) */
  myNonce: number;
  /** Their balance proof nonce (monotonic) */
  theirNonce: number;
  /** Cumulative amount I've sent to them, bigint as string */
  myTransferred: string;
  /** Cumulative amount they've sent to me, bigint as string */
  theirTransferred: string;
}

/**
 * Payment Channel Settled Telemetry Event
 *
 * Emitted when channel settlement completes on-chain.
 * Indicates channel closed and final balances distributed.
 *
 * **BigInt Serialization:** All balance fields are strings (bigint serialized for JSON).
 *
 * **Emission Points:**
 * - ChannelManager.settleAfterChallenge() after PaymentChannelSDK.settleChannel() completes
 * - ChannelManager after detecting ChannelSettled blockchain event
 *
 * **Dashboard Usage:**
 * - NetworkGraph removes channel badge or shows "settled" state
 * - PaymentChannelsPanel updates channel status to 'settled'
 * - TimelineView shows channel settled event with final balances
 *
 * @example
 * ```typescript
 * const event: PaymentChannelSettledEvent = {
 *   type: 'PAYMENT_CHANNEL_SETTLED',
 *   timestamp: '2026-01-09T14:00:00.000Z',
 *   nodeId: 'connector-a',
 *   channelId: '0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef',
 *   finalBalances: {
 *     '0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb': '3000000000000000000',
 *     '0x8626f6940E2eb28930eFb4CeF49B2d1F2C9C1199': '2000000000000000000'
 *   },
 *   settlementType: 'cooperative'
 * };
 * ```
 */
export interface PaymentChannelSettledEvent {
  /** Event type discriminator */
  type: 'PAYMENT_CHANNEL_SETTLED';
  /** Event timestamp (ISO 8601 format) */
  timestamp: string;
  /** Connector node ID emitting event */
  nodeId: string;
  /** bytes32 channel identifier */
  channelId: string;
  /** Final balances by participant address, bigint as string */
  finalBalances: {
    [participant: string]: string;
  };
  /** Settlement method: 'cooperative' (mutual close), 'unilateral' (force close), 'disputed' (challenge period) */
  settlementType: 'cooperative' | 'unilateral' | 'disputed';
}

/**
 * Dashboard Channel State
 *
 * In-memory representation of payment channel for dashboard visualization.
 * Aggregates telemetry events to maintain current channel state.
 *
 * This interface is used by the dashboard backend (TelemetryServer) and frontend
 * (PaymentChannelsPanel, ChannelCard) to track and display channel state.
 *
 * @example
 * ```typescript
 * const channelState: DashboardChannelState = {
 *   channelId: '0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef',
 *   nodeId: 'connector-a',
 *   peerId: 'connector-b',
 *   participants: ['0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb', '0x8626f6940E2eb28930eFb4CeF49B2d1F2C9C1199'],
 *   tokenAddress: '0x5FbDB2315678afecb367f032d93F642f64180aa3',
 *   tokenSymbol: 'USDC',
 *   settlementTimeout: 86400,
 *   deposits: {
 *     '0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb': '1000000000000000000',
 *     '0x8626f6940E2eb28930eFb4CeF49B2d1F2C9C1199': '0'
 *   },
 *   myNonce: 5,
 *   theirNonce: 3,
 *   myTransferred: '5000000000000000000',
 *   theirTransferred: '2000000000000000000',
 *   status: 'active',
 *   openedAt: '2026-01-09T12:00:00.000Z',
 *   lastActivityAt: '2026-01-09T12:01:00.000Z'
 * };
 * ```
 */
export interface DashboardChannelState {
  /** bytes32 channel identifier */
  channelId: string;
  /** Connector node ID */
  nodeId: string;
  /** Peer connector ID */
  peerId: string;
  /** Participant addresses */
  participants: [string, string];
  /** ERC20 token contract address */
  tokenAddress: string;
  /** Human-readable token symbol */
  tokenSymbol: string;
  /** Challenge period duration (seconds) */
  settlementTimeout: number;
  /** Deposits by participant address, bigint as string */
  deposits: {
    [participant: string]: string;
  };
  /** My balance proof nonce */
  myNonce: number;
  /** Their balance proof nonce */
  theirNonce: number;
  /** Cumulative amount sent, bigint as string */
  myTransferred: string;
  /** Cumulative amount received, bigint as string */
  theirTransferred: string;
  /** Channel lifecycle status */
  status: 'opening' | 'active' | 'closing' | 'settling' | 'settled';
  /** ISO 8601 timestamp when opened */
  openedAt: string;
  /** ISO 8601 timestamp when settled (if settled) */
  settledAt?: string;
  /** ISO 8601 timestamp of last balance update */
  lastActivityAt: string;
}
