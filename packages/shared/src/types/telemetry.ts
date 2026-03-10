/**
 * Telemetry Event Type Definitions
 *
 * This module provides TypeScript type definitions for telemetry events emitted
 * by the connector to the dashboard for real-time visualization.
 *
 * Event types support settlement monitoring, account balance tracking, payment
 * channel lifecycle tracking, and network activity visualization.
 *
 * @packageDocumentation
 */

import {
  PaymentChannelOpenedEvent,
  PaymentChannelBalanceUpdateEvent,
  PaymentChannelSettledEvent,
} from './payment-channel-telemetry';

/**
 * Telemetry Event Type Discriminator
 *
 * Enumeration of all telemetry event types emitted by the connector.
 * Each event type corresponds to a specific telemetry event interface.
 */
export enum TelemetryEventType {
  /** Node status event - emitted on startup/shutdown/state change */
  NODE_STATUS = 'NODE_STATUS',
  /** Packet received event - emitted when ILP packet received */
  PACKET_RECEIVED = 'PACKET_RECEIVED',
  /** Packet forwarded event - emitted when ILP packet forwarded */
  PACKET_FORWARDED = 'PACKET_FORWARDED',
  /** Packet fulfilled event - emitted when ILP packet successfully fulfilled */
  PACKET_FULFILLED = 'PACKET_FULFILLED',
  /** Packet rejected event - emitted when ILP packet rejected */
  PACKET_REJECTED = 'PACKET_REJECTED',
  /** Packet sent event - emitted when ILP packet sent to next hop (deprecated, use PACKET_FORWARDED) */
  PACKET_SENT = 'PACKET_SENT',
  /** Account balance event - emitted when account balance changes (Story 6.8) */
  ACCOUNT_BALANCE = 'ACCOUNT_BALANCE',
  /** Settlement triggered event - emitted when settlement threshold exceeded (Story 6.6) */
  SETTLEMENT_TRIGGERED = 'SETTLEMENT_TRIGGERED',
  /** Settlement completed event - emitted when settlement execution completes (Story 6.7) */
  SETTLEMENT_COMPLETED = 'SETTLEMENT_COMPLETED',
  /** Agent balance changed event - emitted when agent wallet balance changes (Story 11.3) */
  AGENT_BALANCE_CHANGED = 'AGENT_BALANCE_CHANGED',
  /** Agent wallet funded event - emitted when agent wallet receives initial funding (Story 11.4) */
  AGENT_WALLET_FUNDED = 'AGENT_WALLET_FUNDED',
  /** Funding rate limit exceeded event - emitted when rate limit hit (Story 11.4) */
  FUNDING_RATE_LIMIT_EXCEEDED = 'FUNDING_RATE_LIMIT_EXCEEDED',
  /** Funding transaction confirmed event - emitted when funding tx confirmed on-chain (Story 11.4) */
  FUNDING_TRANSACTION_CONFIRMED = 'FUNDING_TRANSACTION_CONFIRMED',
  /** Funding transaction failed event - emitted when funding tx fails (Story 11.4) */
  FUNDING_TRANSACTION_FAILED = 'FUNDING_TRANSACTION_FAILED',
  /** Agent wallet state changed event - emitted on wallet lifecycle state transitions (Story 11.5) */
  AGENT_WALLET_STATE_CHANGED = 'AGENT_WALLET_STATE_CHANGED',
  /** Payment channel opened event - emitted when payment channel created on-chain (Story 8.10) */
  PAYMENT_CHANNEL_OPENED = 'PAYMENT_CHANNEL_OPENED',
  /** Payment channel balance update event - emitted when off-chain balance proofs updated (Story 8.10) */
  PAYMENT_CHANNEL_BALANCE_UPDATE = 'PAYMENT_CHANNEL_BALANCE_UPDATE',
  /** Payment channel settled event - emitted when channel settlement completes on-chain (Story 8.10) */
  PAYMENT_CHANNEL_SETTLED = 'PAYMENT_CHANNEL_SETTLED',
  /** Agent payment channel opened event - emitted when agent opens payment channel (Story 11.6) */
  AGENT_CHANNEL_OPENED = 'AGENT_CHANNEL_OPENED',
  /** Agent payment channel payment sent event - emitted when agent sends payment through channel (Story 11.6) */
  AGENT_CHANNEL_PAYMENT_SENT = 'AGENT_CHANNEL_PAYMENT_SENT',
  /** Agent payment channel balance update event - emitted when channel balance changes (Story 11.6) */
  AGENT_CHANNEL_BALANCE_UPDATE = 'AGENT_CHANNEL_BALANCE_UPDATE',
  /** Agent payment channel closed event - emitted when agent closes payment channel (Story 11.6) */
  AGENT_CHANNEL_CLOSED = 'AGENT_CHANNEL_CLOSED',
  /** Wallet balance mismatch event - emitted when backup restore detects balance discrepancy (Story 11.8) */
  WALLET_BALANCE_MISMATCH = 'WALLET_BALANCE_MISMATCH',
  /** Suspicious activity detected event - emitted when fraud/suspicious activity detected (Story 11.9) */
  SUSPICIOUS_ACTIVITY_DETECTED = 'SUSPICIOUS_ACTIVITY_DETECTED',
  /** Rate limit exceeded event - emitted when rate limit exceeded (Story 11.9) */
  RATE_LIMIT_EXCEEDED = 'RATE_LIMIT_EXCEEDED',
  /** Claim sent event - emitted when payment channel claim sent via BTP (Story 17.2) */
  CLAIM_SENT = 'CLAIM_SENT',
  /** Claim received event - emitted when payment channel claim received via BTP (Story 17.3) */
  CLAIM_RECEIVED = 'CLAIM_RECEIVED',
  /** Claim redeemed event - emitted when claim redeemed on-chain (Story 17.5) */
  CLAIM_REDEEMED = 'CLAIM_REDEEMED',
  /** Per-hop notification event - emitted when fire-and-forget BLS notification dispatched (Story 30.3) */
  PER_HOP_NOTIFICATION = 'PER_HOP_NOTIFICATION',
}

/**
 * Settlement State Enumeration
 *
 * Tracks the current state of settlement for a peer account.
 * Used by SettlementMonitor (Story 6.6) to prevent duplicate settlement triggers.
 */
export enum SettlementState {
  /** No settlement in progress, normal operation */
  IDLE = 'IDLE',
  /** Settlement threshold exceeded, settlement queued */
  SETTLEMENT_PENDING = 'SETTLEMENT_PENDING',
  /** Settlement execution in progress */
  SETTLEMENT_IN_PROGRESS = 'SETTLEMENT_IN_PROGRESS',
}

/**
 * Account Balance Telemetry Event
 *
 * Emitted whenever an account balance changes due to packet forwarding or settlement.
 * Sent by AccountManager (Story 6.3) after recordPacketTransfers() or recordSettlement().
 *
 * **BigInt Serialization:** All balance fields are strings (bigint values serialized as
 * strings for JSON compatibility). Use `BigInt(value)` to convert back to bigint.
 *
 * **Emission Points:**
 * - After packet forward: AccountManager.recordPacketTransfers()
 * - After settlement: AccountManager.recordSettlement()
 *
 * **Dashboard Usage:**
 * - SettlementStatusPanel displays balance table with color-coded thresholds
 * - NetworkGraph shows balance badges on peer nodes
 * - SettlementTimeline tracks balance changes over time
 *
 * @example
 * ```typescript
 * const event: AccountBalanceEvent = {
 *   type: 'ACCOUNT_BALANCE',
 *   nodeId: 'connector-a',
 *   peerId: 'peer-b',
 *   tokenId: 'M2M',
 *   debitBalance: '0',
 *   creditBalance: '1000',
 *   netBalance: '-1000',
 *   creditLimit: '10000',
 *   settlementThreshold: '5000',
 *   settlementState: SettlementState.IDLE,
 *   timestamp: '2026-01-03T12:00:00.000Z'
 * };
 * ```
 */
export interface AccountBalanceEvent {
  /** Event type discriminator */
  type: 'ACCOUNT_BALANCE';
  /** Connector node ID emitting this event */
  nodeId: string;
  /** Peer account ID (connector peered with) */
  peerId: string;
  /** Token ID (e.g., 'M2M', 'ETH') */
  tokenId: string;
  /** Debit balance (amount we owe peer), bigint as string */
  debitBalance: string;
  /** Credit balance (amount peer owes us), bigint as string */
  creditBalance: string;
  /** Net balance (debitBalance - creditBalance), bigint as string */
  netBalance: string;
  /** Credit limit (max peer can owe us), bigint as string, optional */
  creditLimit?: string;
  /** Settlement threshold (balance triggers settlement), bigint as string, optional */
  settlementThreshold?: string;
  /** Current settlement state */
  settlementState: SettlementState;
  /** Event timestamp (ISO 8601 format) */
  timestamp: string;
}

/**
 * Settlement Triggered Telemetry Event
 *
 * Emitted when SettlementMonitor (Story 6.6) detects a settlement threshold crossing.
 * Indicates that a settlement has been queued for execution.
 *
 * **Trigger Conditions:**
 * - Threshold exceeded: creditBalance >= settlementThreshold
 * - Manual trigger: Operator manually triggers settlement via API
 *
 * **BigInt Serialization:** All balance fields are strings (bigint serialized for JSON).
 *
 * **Dashboard Usage:**
 * - SettlementTimeline shows trigger event with threshold details
 * - SettlementStatusPanel updates peer state to SETTLEMENT_PENDING
 *
 * @example
 * ```typescript
 * const event: SettlementTriggeredEvent = {
 *   type: 'SETTLEMENT_TRIGGERED',
 *   nodeId: 'connector-a',
 *   peerId: 'peer-b',
 *   tokenId: 'M2M',
 *   currentBalance: '5500',
 *   threshold: '5000',
 *   exceedsBy: '500',
 *   triggerReason: 'THRESHOLD_EXCEEDED',
 *   timestamp: '2026-01-03T12:00:00.000Z'
 * };
 * ```
 */
export interface SettlementTriggeredEvent {
  /** Event type discriminator */
  type: 'SETTLEMENT_TRIGGERED';
  /** Connector node ID triggering settlement */
  nodeId: string;
  /** Peer account ID requiring settlement */
  peerId: string;
  /** Token ID */
  tokenId: string;
  /** Current balance when triggered, bigint as string */
  currentBalance: string;
  /** Settlement threshold that was exceeded, bigint as string */
  threshold: string;
  /** Amount over threshold (currentBalance - threshold), bigint as string */
  exceedsBy: string;
  /** Trigger reason: 'THRESHOLD_EXCEEDED' (automatic) or 'MANUAL' (operator-initiated) */
  triggerReason: string;
  /** Event timestamp (ISO 8601 format) */
  timestamp: string;
}

/**
 * Settlement Completed Telemetry Event
 *
 * Emitted when SettlementAPI (Story 6.7) completes settlement execution.
 * Reports the settlement outcome (success/failure) and balance changes.
 *
 * **Settlement Types:**
 * - 'MOCK': Mock settlement (Story 6.7) - TigerBeetle transfer only, no blockchain
 * - 'EVM': Ethereum settlement (Epic 7) - EVM blockchain payment
 * - 'XRP': XRP Ledger settlement (Epic 8) - XRP Ledger payment
 *
 * **BigInt Serialization:** All balance fields are strings (bigint serialized for JSON).
 *
 * **Dashboard Usage:**
 * - SettlementTimeline shows completion event with success/failure indicator
 * - SettlementStatusPanel updates peer balance to newBalance
 * - NetworkGraph updates balance badges to reflect settlement
 *
 * @example
 * ```typescript
 * // Successful settlement
 * const successEvent: SettlementCompletedEvent = {
 *   type: 'SETTLEMENT_COMPLETED',
 *   nodeId: 'connector-a',
 *   peerId: 'peer-b',
 *   tokenId: 'M2M',
 *   previousBalance: '5500',
 *   newBalance: '0',
 *   settledAmount: '5500',
 *   settlementType: 'MOCK',
 *   success: true,
 *   timestamp: '2026-01-03T12:01:00.000Z'
 * };
 *
 * // Failed settlement
 * const failureEvent: SettlementCompletedEvent = {
 *   type: 'SETTLEMENT_COMPLETED',
 *   nodeId: 'connector-a',
 *   peerId: 'peer-b',
 *   tokenId: 'M2M',
 *   previousBalance: '5500',
 *   newBalance: '5500',
 *   settledAmount: '0',
 *   settlementType: 'MOCK',
 *   success: false,
 *   errorMessage: 'TigerBeetle transfer failed: insufficient balance',
 *   timestamp: '2026-01-03T12:01:00.000Z'
 * };
 * ```
 */
export interface SettlementCompletedEvent {
  /** Event type discriminator */
  type: 'SETTLEMENT_COMPLETED';
  /** Connector node ID completing settlement */
  nodeId: string;
  /** Peer account ID settled with */
  peerId: string;
  /** Token ID */
  tokenId: string;
  /** Balance before settlement, bigint as string */
  previousBalance: string;
  /** Balance after settlement, bigint as string */
  newBalance: string;
  /** Amount settled (previousBalance - newBalance), bigint as string */
  settledAmount: string;
  /** Settlement type: 'MOCK' (Story 6.7), 'EVM' (Epic 7), 'XRP' (Epic 8) */
  settlementType: string;
  /** Settlement execution result: true=success, false=failure */
  success: boolean;
  /** Error message if success=false, undefined if success=true */
  errorMessage?: string;
  /** Event timestamp (ISO 8601 format) */
  timestamp: string;
}

/**
 * Packet Received Telemetry Event
 *
 * Emitted when PacketHandler receives an ILP Prepare packet.
 * Indicates an ILP packet has been received from an upstream peer or client.
 *
 * **Dashboard Usage:**
 * - Explorer UI displays packet flow visualization
 * - Packet inspector shows packet details
 *
 * @example
 * ```typescript
 * const event: PacketReceivedEvent = {
 *   type: 'PACKET_RECEIVED',
 *   nodeId: 'connector-a',
 *   packetId: 'abc123...',
 *   destination: 'g.connector.peer2',
 *   amount: '1000',
 *   from: 'peer1',
 *   timestamp: 1704729600000
 * };
 * ```
 */
export interface PacketReceivedEvent {
  /** Event type discriminator */
  type: 'PACKET_RECEIVED';
  /** Connector node ID receiving packet */
  nodeId: string;
  /** Packet identifier (execution condition hex) */
  packetId: string;
  /** ILP destination address */
  destination: string;
  /** Packet amount, bigint as string */
  amount: string;
  /** Peer ID who sent the packet */
  from: string;
  /** Event timestamp (Unix milliseconds) */
  timestamp: number;
}

/**
 * Packet Forwarded Telemetry Event
 *
 * Emitted when PacketHandler forwards an ILP packet to the next hop.
 * Indicates an ILP packet has been successfully forwarded to a downstream peer.
 *
 * **Dashboard Usage:**
 * - Explorer UI displays packet flow visualization
 * - Packet inspector shows forwarding decisions
 *
 * @example
 * ```typescript
 * const event: PacketForwardedEvent = {
 *   type: 'PACKET_FORWARDED',
 *   nodeId: 'connector-a',
 *   packetId: 'abc123...',
 *   destination: 'g.connector.peer2',
 *   amount: '990',
 *   to: 'peer2',
 *   timestamp: 1704729600000
 * };
 * ```
 */
export interface PacketForwardedEvent {
  /** Event type discriminator */
  type: 'PACKET_FORWARDED';
  /** Connector node ID forwarding packet */
  nodeId: string;
  /** Packet identifier (execution condition hex) */
  packetId: string;
  /** ILP destination address */
  destination: string;
  /** Forwarded packet amount (after fee), bigint as string */
  amount: string;
  /** Peer ID to whom the packet was forwarded */
  to: string;
  /** Event timestamp (Unix milliseconds) */
  timestamp: number;
}

/**
 * Packet Fulfilled Telemetry Event
 *
 * Emitted when an ILP Prepare packet is successfully fulfilled.
 */
export interface PacketFulfilledEvent {
  /** Event type discriminator */
  type: 'PACKET_FULFILLED';
  /** Connector node ID */
  nodeId: string;
  /** Packet identifier (execution condition hex) */
  packetId: string;
  /** ILP destination address */
  destination: string;
  /** Packet amount, bigint as string */
  amount: string;
  /** Source peer ID */
  from: string;
  /** Fulfillment (32-byte preimage hex) */
  fulfillment: string;
  /** Event timestamp (Unix milliseconds) */
  timestamp: number;
}

/**
 * Packet Rejected Telemetry Event
 *
 * Emitted when an ILP Prepare packet is rejected.
 */
export interface PacketRejectedEvent {
  /** Event type discriminator */
  type: 'PACKET_REJECTED';
  /** Connector node ID */
  nodeId: string;
  /** Packet identifier (execution condition hex) */
  packetId: string;
  /** ILP destination address */
  destination: string;
  /** Packet amount, bigint as string */
  amount: string;
  /** Source peer ID */
  from: string;
  /** ILP error code */
  code: string;
  /** Rejection message */
  message: string;
  /** Event timestamp (Unix milliseconds) */
  timestamp: number;
}

/**
 * Agent Balance Changed Telemetry Event
 *
 * Emitted when AgentBalanceTracker (Story 11.3) detects a balance change for an agent wallet.
 * Indicates on-chain balance has increased or decreased.
 *
 * **BigInt Serialization:** All balance fields are strings (bigint serialized for JSON).
 *
 * **Dashboard Usage:**
 * - Story 11.7 dashboard displays real-time balance updates
 * - Story 11.4 funding logic subscribes to detect low balances
 *
 * @example
 * ```typescript
 * const event: AgentBalanceChangedEvent = {
 *   type: 'AGENT_BALANCE_CHANGED',
 *   agentId: 'agent-001',
 *   chain: 'evm',
 *   token: 'ETH',
 *   oldBalance: '1000000000000000000',
 *   newBalance: '2000000000000000000',
 *   change: '1000000000000000000',
 *   timestamp: 1704729600000
 * };
 * ```
 */
export interface AgentBalanceChangedEvent {
  /** Event type discriminator */
  type: 'AGENT_BALANCE_CHANGED';
  /** Agent identifier */
  agentId: string;
  /** Blockchain ('evm') */
  chain: string;
  /** Token identifier ('ETH' or ERC20 address) */
  token: string;
  /** Previous balance, bigint as string */
  oldBalance: string;
  /** New balance, bigint as string */
  newBalance: string;
  /** Balance change (newBalance - oldBalance), bigint as string */
  change: string;
  /** Event timestamp (ISO 8601 format) */
  timestamp: string;
}

/**
 * Funding Transaction Interface
 *
 * Represents a single funding transaction (ETH or ERC20).
 * Used by AgentWalletFundedEvent (Story 11.4).
 */
export interface FundingTransaction {
  /** Blockchain ('evm') */
  chain: 'evm';
  /** Token identifier ('ETH', ERC20 address, or 'XRP') */
  token: string;
  /** Recipient address */
  to: string;
  /** Amount as string (bigint serialized) */
  amount: string;
  /** Transaction hash for on-chain lookup */
  txHash: string;
  /** Transaction status */
  status: 'pending' | 'confirmed' | 'failed';
}

/**
 * Agent Wallet Funded Telemetry Event
 *
 * Emitted when AgentWalletFunder (Story 11.4) successfully funds a new agent wallet.
 * Indicates agent received initial ETH and ERC20 token funding.
 *
 * **BigInt Serialization:** All amount fields in transactions are strings (bigint serialized for JSON).
 *
 * **Dashboard Usage:**
 * - Story 11.7 dashboard displays funding events in real-time
 * - Funding history panel shows transaction details
 *
 * @example
 * ```typescript
 * const event: AgentWalletFundedEvent = {
 *   type: 'AGENT_WALLET_FUNDED',
 *   agentId: 'agent-001',
 *   evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb',
 *   xrpAddress: 'rN7n7otQDd6FczFgLdlqtyMVrn3WnFBrJT',
 *   transactions: [
 *     { chain: 'evm', token: 'ETH', to: '0x742d35Cc...', amount: '10000000000000000', txHash: '0xabc...', status: 'pending' },
 *     { chain: 'xrp', token: 'XRP', to: 'rN7n7otQDd...', amount: '15000000', txHash: 'ABC123...', status: 'pending' }
 *   ],
 *   timestamp: '2026-01-08T12:00:00.000Z'
 * };
 * ```
 */
export interface AgentWalletFundedEvent {
  /** Event type discriminator */
  type: 'AGENT_WALLET_FUNDED';
  /** Agent identifier */
  agentId: string;
  /** Agent EVM address */
  evmAddress: string;
  /** List of funding transactions */
  transactions: FundingTransaction[];
  /** Event timestamp (ISO 8601 format) */
  timestamp: string;
}

/**
 * Funding Rate Limit Exceeded Telemetry Event
 *
 * Emitted when AgentWalletFunder (Story 11.4) denies funding due to rate limit.
 * Indicates potential abuse or misconfiguration.
 *
 * @example
 * ```typescript
 * const event: FundingRateLimitExceededEvent = {
 *   type: 'FUNDING_RATE_LIMIT_EXCEEDED',
 *   agentId: 'agent-001',
 *   violatedLimit: 'per_agent',
 *   timestamp: '2026-01-08T12:00:00.000Z'
 * };
 * ```
 */
export interface FundingRateLimitExceededEvent {
  /** Event type discriminator */
  type: 'FUNDING_RATE_LIMIT_EXCEEDED';
  /** Agent identifier */
  agentId: string;
  /** Which rate limit was violated */
  violatedLimit: 'per_agent' | 'per_hour';
  /** Event timestamp (ISO 8601 format) */
  timestamp: string;
}

/**
 * Funding Transaction Confirmed Telemetry Event
 *
 * Emitted when AgentWalletFunder (Story 11.4) confirms funding transaction on-chain.
 *
 * @example
 * ```typescript
 * const event: FundingTransactionConfirmedEvent = {
 *   type: 'FUNDING_TRANSACTION_CONFIRMED',
 *   agentId: 'agent-001',
 *   txHash: '0xabc123...',
 *   chain: 'evm',
 *   status: 'confirmed',
 *   timestamp: '2026-01-08T12:01:00.000Z'
 * };
 * ```
 */
export interface FundingTransactionConfirmedEvent {
  /** Event type discriminator */
  type: 'FUNDING_TRANSACTION_CONFIRMED';
  /** Agent identifier */
  agentId: string;
  /** Transaction hash */
  txHash: string;
  /** Blockchain ('evm') */
  chain: string;
  /** Transaction status */
  status: 'confirmed';
  /** Event timestamp (ISO 8601 format) */
  timestamp: string;
}

/**
 * Funding Transaction Failed Telemetry Event
 *
 * Emitted when AgentWalletFunder (Story 11.4) detects funding transaction failure.
 *
 * @example
 * ```typescript
 * const event: FundingTransactionFailedEvent = {
 *   type: 'FUNDING_TRANSACTION_FAILED',
 *   agentId: 'agent-001',
 *   txHash: '0xabc123...',
 *   chain: 'evm',
 *   error: 'Transaction reverted',
 *   timestamp: '2026-01-08T12:01:00.000Z'
 * };
 * ```
 */
export interface FundingTransactionFailedEvent {
  /** Event type discriminator */
  type: 'FUNDING_TRANSACTION_FAILED';
  /** Agent identifier */
  agentId: string;
  /** Transaction hash */
  txHash: string;
  /** Blockchain ('evm') */
  chain: string;
  /** Error message */
  error: string;
  /** Event timestamp (ISO 8601 format) */
  timestamp: string;
}

/**
 * Agent Wallet State Changed Telemetry Event
 *
 * Emitted when AgentWalletLifecycle (Story 11.5) transitions wallet state.
 * Indicates wallet lifecycle progression (PENDING → ACTIVE → SUSPENDED → ARCHIVED).
 *
 * **Dashboard Usage:**
 * - Story 11.7 dashboard displays lifecycle state badges on agent wallet cards
 * - Real-time state transition visualization
 *
 * @example
 * ```typescript
 * const event: AgentWalletStateChangedEvent = {
 *   type: 'AGENT_WALLET_STATE_CHANGED',
 *   agentId: 'agent-001',
 *   oldState: 'pending',
 *   newState: 'active',
 *   timestamp: 1704729600000
 * };
 * ```
 */
export interface AgentWalletStateChangedEvent {
  /** Event type discriminator */
  type: 'AGENT_WALLET_STATE_CHANGED';
  /** Agent identifier */
  agentId: string;
  /** Previous state (null if newly created) */
  oldState: string | null;
  /** New state */
  newState: string;
  /** Event timestamp (Unix milliseconds) */
  timestamp: number;
}

/**
 * XRP Channel Opened Telemetry Event
 *
 * Emitted when XRPChannelSDK.openChannel() successfully creates payment channel.
 * Indicates XRP payment channel has been created on the XRP Ledger.
 *
 * **BigInt Serialization:** All XRP amount fields are strings (bigint serialized for JSON).
 * XRP amounts stored in "drops" (1 XRP = 1,000,000 drops).
 *
 * **Dashboard Usage:**
 * - PaymentChannelsPanel displays XRP channels with orange badge
 * - ChannelTimeline shows channel opened event
 * - NetworkGraph displays XRP channel indicator
 *
 * @example
 * ```typescript
 * const event: XRPChannelOpenedEvent = {
 *   type: 'XRP_CHANNEL_OPENED',
 *   timestamp: '2026-01-12T12:00:00.000Z',
 *   nodeId: 'connector-a',
 *   channelId: 'A1B2C3D4E5F6789...',
 *   account: 'rN7n7otQDd6FczFgLdlqtyMVrn3HMfXEEW',
 *   destination: 'rLHzPsX6oXkzU9rFkRaYT8yBqJcQwPgHWN',
 *   amount: '10000000000',
 *   settleDelay: 86400,
 *   publicKey: 'ED01234567890ABCDEF...',
 *   peerId: 'peer-bob'
 * };
 * ```
 */
export interface XRPChannelOpenedEvent {
  /**
   * Event type discriminator
   */
  type: 'XRP_CHANNEL_OPENED';

  /**
   * Event timestamp (ISO 8601 format)
   * Format: '2026-01-12T12:00:00.000Z'
   */
  timestamp: string;

  /**
   * Connector node ID emitting event
   * Example: 'connector-a'
   */
  nodeId: string;

  /**
   * XRP payment channel identifier (transaction hash)
   * Format: 64-character hex string
   * Example: 'A1B2C3D4E5F6...'
   */
  channelId: string;

  /**
   * Source account (channel sender, us)
   * Format: XRP Ledger r-address
   * Example: 'rN7n7otQDd6FczFgLdlqtyMVrn3HMfXEEW'
   */
  account: string;

  /**
   * Destination account (channel recipient, peer)
   * Format: XRP Ledger r-address
   * Example: 'rLHzPsX6oXkzU9rFkRaYT8yBqJcQwPgHWN'
   */
  destination: string;

  /**
   * Total XRP deposited in channel (drops)
   * Format: String for bigint precision
   * Example: '10000000000' = 10,000 XRP (1 XRP = 1,000,000 drops)
   */
  amount: string;

  /**
   * Settlement delay in seconds
   * Example: 86400 (24 hours)
   */
  settleDelay: number;

  /**
   * ed25519 public key for claim signature verification
   * Format: 66-character hex string (ED prefix + 64 hex)
   * Example: 'ED01234567890ABCDEF...'
   */
  publicKey: string;

  /**
   * Peer identifier from connector configuration
   * Example: 'peer-bob'
   */
  peerId?: string;
}

/**
 * XRP Channel Claimed Telemetry Event
 *
 * Emitted when XRPChannelSDK.submitClaim() redeems XRP from channel.
 * Indicates XRP has been claimed from the payment channel on the XRP Ledger.
 *
 * **BigInt Serialization:** All XRP amount fields are strings (bigint serialized for JSON).
 *
 * **Dashboard Usage:**
 * - ChannelTimeline shows claim submission event
 * - PaymentChannelsPanel updates XRP channel balance
 *
 * @example
 * ```typescript
 * const event: XRPChannelClaimedEvent = {
 *   type: 'XRP_CHANNEL_CLAIMED',
 *   timestamp: '2026-01-12T12:05:00.000Z',
 *   nodeId: 'connector-a',
 *   channelId: 'A1B2C3D4E5F6789...',
 *   claimAmount: '5000000000',
 *   remainingBalance: '5000000000',
 *   peerId: 'peer-bob'
 * };
 * ```
 */
export interface XRPChannelClaimedEvent {
  /**
   * Event type discriminator
   */
  type: 'XRP_CHANNEL_CLAIMED';

  /**
   * Event timestamp (ISO 8601 format)
   */
  timestamp: string;

  /**
   * Connector node ID emitting event
   */
  nodeId: string;

  /**
   * XRP payment channel identifier (transaction hash)
   * Format: 64-character hex string
   */
  channelId: string;

  /**
   * XRP claimed in this claim transaction (cumulative drops)
   * Format: String for bigint precision
   * Example: '5000000000' = 5,000 XRP claimed total
   */
  claimAmount: string;

  /**
   * XRP remaining in channel after claim (drops)
   * Format: String for bigint precision
   * Calculation: channel.amount - claimAmount
   * Example: '5000000000' = 5,000 XRP remaining
   */
  remainingBalance: string;

  /**
   * Peer identifier from connector configuration
   * Example: 'peer-bob'
   */
  peerId?: string;
}

/**
 * XRP Channel Closed Telemetry Event
 *
 * Emitted when XRPChannelSDK.closeChannel() initiates or finalizes closure.
 * Indicates XRP payment channel closure has been initiated on the XRP Ledger.
 *
 * **BigInt Serialization:** All XRP amount fields are strings (bigint serialized for JSON).
 *
 * **Dashboard Usage:**
 * - ChannelTimeline shows channel closed event
 * - PaymentChannelsPanel marks channel as settled
 *
 * @example
 * ```typescript
 * const event: XRPChannelClosedEvent = {
 *   type: 'XRP_CHANNEL_CLOSED',
 *   timestamp: '2026-01-12T12:10:00.000Z',
 *   nodeId: 'connector-a',
 *   channelId: 'A1B2C3D4E5F6789...',
 *   finalBalance: '5000000000',
 *   closeType: 'cooperative',
 *   peerId: 'peer-bob'
 * };
 * ```
 */
export interface XRPChannelClosedEvent {
  /**
   * Event type discriminator
   */
  type: 'XRP_CHANNEL_CLOSED';

  /**
   * Event timestamp (ISO 8601 format)
   */
  timestamp: string;

  /**
   * Connector node ID emitting event
   */
  nodeId: string;

  /**
   * XRP payment channel identifier (transaction hash)
   * Format: 64-character hex string
   */
  channelId: string;

  /**
   * Final XRP distributed when channel closed (drops)
   * Format: String for bigint precision
   * Example: '5000000000' = 5,000 XRP distributed to destination
   */
  finalBalance: string;

  /**
   * Channel closure method
   * - 'cooperative': Both parties agreed to close (closeChannel())
   * - 'expiration': Channel auto-expired via CancelAfter timestamp
   * - 'unilateral': One party closed during settle delay
   */
  closeType: 'cooperative' | 'expiration' | 'unilateral';

  /**
   * Peer identifier from connector configuration
   * Example: 'peer-bob'
   */
  peerId?: string;
}

/**
 * Agent Channel Opened Telemetry Event
 *
 * Emitted when AgentChannelManager (Story 11.6) opens payment channel for agent.
 * Indicates agent has opened a payment channel (EVM or XRP) for micropayments.
 *
 * **BigInt Serialization:** All amount fields are strings (bigint serialized for JSON).
 *
 * **Dashboard Usage:**
 * - Story 11.7 dashboard displays agent channel state and activity
 * - Real-time channel opened events visualization
 *
 * @example
 * ```typescript
 * const event: AgentChannelOpenedEvent = {
 *   type: 'AGENT_CHANNEL_OPENED',
 *   timestamp: 1704729600000,
 *   nodeId: 'connector-a',
 *   agentId: 'agent-001',
 *   channelId: '0xabc123...',
 *   chain: 'evm',
 *   peerId: 'agent-002',
 *   amount: '1000000000000000000'
 * };
 * ```
 */
export interface AgentChannelOpenedEvent {
  /** Event type discriminator */
  type: 'AGENT_CHANNEL_OPENED';
  /** Event timestamp (Unix milliseconds) */
  timestamp: number;
  /** Connector node ID emitting event */
  nodeId: string;
  /** Agent identifier */
  agentId: string;
  /** Channel ID (EVM: bytes32) */
  channelId: string;
  /** Blockchain network ('evm') */
  chain: 'evm';
  /** Peer agent identifier */
  peerId: string;
  /** Initial deposit amount, bigint as string */
  amount: string;
}

/**
 * ILP Packet Type enumeration
 *
 * Represents the three types of ILP packets:
 * - PREPARE: Initial packet sent to begin a payment
 * - FULFILL: Response indicating successful payment completion
 * - REJECT: Response indicating payment failure
 */
export type IlpPacketType = 'prepare' | 'fulfill' | 'reject';

/**
 * Agent Channel Payment Sent Telemetry Event
 *
 * Emitted when AgentChannelManager (Story 11.6) sends payment through channel.
 * Indicates agent has sent off-chain payment via balance proof/claim.
 *
 * **ILP Packet Semantics:**
 * - packetType: 'prepare', 'fulfill', or 'reject' (ILP packet type)
 * - from: The sender of this packet (who originated it)
 * - to: The next hop (immediate peer receiving this packet)
 * - destination: The full ILP address destination (final recipient)
 *
 * **BigInt Serialization:** All amount fields are strings (bigint serialized for JSON).
 *
 * **Dashboard Usage:**
 * - Story 11.7 dashboard displays channel payment activity
 * - Real-time payment flow visualization
 * - Explorer UI shows ILP packet routing with from/to/destination
 *
 * @example
 * ```typescript
 * const event: AgentChannelPaymentSentEvent = {
 *   type: 'AGENT_CHANNEL_PAYMENT_SENT',
 *   timestamp: 1704729660000,
 *   nodeId: 'connector-a',
 *   agentId: 'agent-001',
 *   packetType: 'prepare',
 *   packetId: 'abc123def456...',
 *   from: 'g.agent.agent-001',
 *   to: 'peer-002',
 *   channelId: '0xabc123...',
 *   amount: '100000000000000000',
 *   destination: 'g.agent.peer-003.final-destination',
 *   event: {
 *     id: 'abc123...',
 *     pubkey: 'def456...',
 *     kind: 1,
 *     content: 'Hello world',
 *     created_at: 1704729600,
 *     tags: [['p', 'recipient-pubkey']],
 *     sig: 'sig789...'
 *   }
 * };
 * ```
 */
export interface AgentChannelPaymentSentEvent {
  /** Event type discriminator */
  type: 'AGENT_CHANNEL_PAYMENT_SENT';
  /** Event timestamp (Unix milliseconds) */
  timestamp: number;
  /** Connector node ID emitting event */
  nodeId: string;
  /** Agent identifier */
  agentId: string;
  /** ILP packet type: 'prepare', 'fulfill', or 'reject' */
  packetType: IlpPacketType;
  /** Unique packet identifier for correlating PREPARE with FULFILL/REJECT responses */
  packetId?: string;
  /** Sender of this packet (who originated it) - ILP address or agent ID */
  from: string;
  /** Next hop (immediate peer receiving this packet) - peer ID */
  to: string;
  /** Peer ID (deprecated, use 'to' instead) */
  peerId?: string;
  /** Channel ID */
  channelId: string;
  /** Payment amount, bigint as string */
  amount: string;
  /** Full ILP destination address (final recipient) */
  destination: string;
  /** Decoded Nostr event from ILP packet data */
  event?: {
    /** Event ID (32-byte hex SHA-256) */
    id: string;
    /** Author public key (32-byte hex) */
    pubkey: string;
    /** Event kind (1=note, 3=follows, etc.) */
    kind: number;
    /** Event content */
    content: string;
    /** Unix timestamp in seconds */
    created_at: number;
    /** Event tags */
    tags: string[][];
    /** Schnorr signature (64-byte hex) */
    sig: string;
  };
  /** ILP packet execution condition (32-byte hex) */
  executionCondition?: string;
  /** ILP packet expiry timestamp */
  expiresAt?: string;
  /** Fulfillment if packet was fulfilled (32-byte hex) */
  fulfillment?: string;
  /** Error code if packet was rejected (e.g., 'F00', 'T01') */
  errorCode?: string;
  /** Error message if packet was rejected */
  errorMessage?: string;
  /** Channel type (evm) */
  channelType?: 'evm' | 'none';
  /** Channel balance after this payment, bigint as string */
  channelBalance?: string;
  /** Channel total deposit, bigint as string */
  channelDeposit?: string;
}

/**
 * Agent Channel Balance Update Telemetry Event
 *
 * Emitted when payment channel balance changes due to packet forwarding.
 * Tracks cumulative balance changes for debugging and monitoring.
 *
 * **BigInt Serialization:** All amount fields are strings (bigint serialized for JSON).
 *
 * **Dashboard Usage:**
 * - Explorer UI shows channel balance progression
 * - Payment channel health monitoring
 *
 * @example
 * ```typescript
 * const event: AgentChannelBalanceUpdateEvent = {
 *   type: 'AGENT_CHANNEL_BALANCE_UPDATE',
 *   timestamp: 1704729660000,
 *   nodeId: 'connector-a',
 *   agentId: 'agent-001',
 *   channelId: '0xabc123...',
 *   channelType: 'evm',
 *   peerId: 'peer-002',
 *   previousBalance: '100',
 *   newBalance: '200',
 *   amount: '100',
 *   direction: 'outgoing',
 *   deposit: '10000000000000000000'
 * };
 * ```
 */
export interface AgentChannelBalanceUpdateEvent {
  /** Event type discriminator */
  type: 'AGENT_CHANNEL_BALANCE_UPDATE';
  /** Event timestamp (Unix milliseconds) */
  timestamp: number;
  /** Connector node ID emitting event */
  nodeId: string;
  /** Agent identifier */
  agentId: string;
  /** Channel ID */
  channelId: string;
  /** Channel type: 'evm' */
  channelType: 'evm';
  /** Peer identifier */
  peerId: string;
  /** Previous balance before this update, bigint as string */
  previousBalance: string;
  /** New balance after this update, bigint as string */
  newBalance: string;
  /** Amount of this update, bigint as string */
  amount: string;
  /** Direction: 'incoming' or 'outgoing' */
  direction: 'incoming' | 'outgoing';
  /** Total channel deposit, bigint as string */
  deposit: string;
}

/**
 * Agent Channel Closed Telemetry Event
 *
 * Emitted when AgentChannelManager (Story 11.6) closes payment channel.
 * Indicates agent has closed channel on-chain.
 *
 * **Dashboard Usage:**
 * - Story 11.7 dashboard displays channel closed events
 * - Channel lifecycle visualization
 *
 * @example
 * ```typescript
 * const event: AgentChannelClosedEvent = {
 *   type: 'AGENT_CHANNEL_CLOSED',
 *   timestamp: 1704729720000,
 *   nodeId: 'connector-a',
 *   agentId: 'agent-001',
 *   channelId: '0xabc123...',
 *   chain: 'evm'
 * };
 * ```
 */
export interface AgentChannelClosedEvent {
  /** Event type discriminator */
  type: 'AGENT_CHANNEL_CLOSED';
  /** Event timestamp (Unix milliseconds) */
  timestamp: number;
  /** Connector node ID emitting event */
  nodeId: string;
  /** Agent identifier */
  agentId: string;
  /** Channel ID */
  channelId: string;
  /** Blockchain network ('evm') */
  chain: 'evm';
}

/**
 * Wallet Balance Mismatch Telemetry Event
 *
 * Emitted when WalletBackupManager (Story 11.8) detects balance mismatch during restore.
 * Indicates backed-up balance differs from actual on-chain balance after recovery.
 *
 * **BigInt Serialization:** All balance fields are strings (bigint serialized for JSON).
 *
 * **Dashboard Usage:**
 * - Story 11.7 dashboard displays balance mismatch warnings
 * - Alerts operators to investigate balance drift
 *
 * @example
 * ```typescript
 * const event: WalletBalanceMismatchEvent = {
 *   type: 'WALLET_BALANCE_MISMATCH',
 *   timestamp: 1704729600000,
 *   nodeId: 'connector-a',
 *   agentId: 'agent-001',
 *   chain: 'evm',
 *   token: 'ETH',
 *   expectedBalance: '1000000000000000000',
 *   actualBalance: '1100000000000000000'
 * };
 * ```
 */
export interface WalletBalanceMismatchEvent {
  /** Event type discriminator */
  type: 'WALLET_BALANCE_MISMATCH';
  /** Event timestamp (Unix milliseconds) */
  timestamp: number;
  /** Connector node ID emitting event */
  nodeId: string;
  /** Agent identifier */
  agentId: string;
  /** Blockchain network ('evm') */
  chain: 'evm';
  /** Token identifier (e.g., 'ETH', '0xUSDC...') */
  token: string;
  /** Expected balance from backup snapshot, bigint as string */
  expectedBalance: string;
  /** Actual on-chain balance after restore, bigint as string */
  actualBalance: string;
}

/**
 * Suspicious Activity Detected Telemetry Event
 *
 * Emitted when SuspiciousActivityDetector (Story 11.9) detects fraud patterns.
 * Indicates rapid funding requests or unusual transaction patterns.
 *
 * **Dashboard Usage:**
 * - Story 11.7 dashboard displays security alerts
 * - Security monitoring panel shows suspicious activity
 *
 * @example
 * ```typescript
 * const event: SuspiciousActivityDetectedEvent = {
 *   type: 'SUSPICIOUS_ACTIVITY_DETECTED',
 *   timestamp: 1704729600000,
 *   nodeId: 'connector-a',
 *   agentId: 'agent-001',
 *   activityType: 'rapid_funding',
 *   details: { fundingCount: 10, threshold: 5 }
 * };
 * ```
 */
export interface SuspiciousActivityDetectedEvent {
  /** Event type discriminator */
  type: 'SUSPICIOUS_ACTIVITY_DETECTED';
  /** Event timestamp (Unix milliseconds) */
  timestamp: number;
  /** Connector node ID emitting event */
  nodeId: string;
  /** Agent identifier */
  agentId: string;
  /** Activity type: rapid funding or unusual transaction */
  activityType: 'rapid_funding' | 'unusual_transaction';
  /** Activity-specific details */
  details: Record<string, unknown>;
}

/**
 * Rate Limit Exceeded Telemetry Event
 *
 * Emitted when RateLimiter (Story 11.9) blocks operation due to rate limit.
 * Indicates potential abuse or DoS attack.
 *
 * **Dashboard Usage:**
 * - Story 11.7 dashboard displays rate limit violations
 * - Security monitoring panel shows blocked operations
 *
 * @example
 * ```typescript
 * const event: RateLimitExceededEvent = {
 *   type: 'RATE_LIMIT_EXCEEDED',
 *   timestamp: 1704729600000,
 *   nodeId: 'connector-a',
 *   operation: 'wallet_creation',
 *   identifier: 'agent-001',
 *   limit: 100
 * };
 * ```
 */
export interface RateLimitExceededEvent {
  /** Event type discriminator */
  type: 'RATE_LIMIT_EXCEEDED';
  /** Event timestamp (Unix milliseconds) */
  timestamp: number;
  /** Connector node ID emitting event */
  nodeId: string;
  /** Operation type that was rate limited */
  operation: string;
  /** Identifier that exceeded limit (agent ID, IP, etc.) */
  identifier: string;
  /** Rate limit (operations/hour) */
  limit: number;
}

/**
 * Claim Settlement Initiated Telemetry Event
 *
 * Emitted when automatic settlement execution begins for a payment channel.
 * Indicates settlement triggered by threshold exceeded, now executing on-chain settlement.
 *
 * **Dashboard Usage:**
 * - Explorer UI shows settlement execution lifecycle
 * - Settlement monitoring panel displays in-progress settlements
 *
 * @example
 * ```typescript
 * const event: ClaimSettlementInitiatedEvent = {
 *   type: 'CLAIM_SETTLEMENT_INITIATED',
 *   nodeId: 'connector-a',
 *   chain: 'evm',
 *   channelId: '0xabc123...',
 *   amount: '5000000000000000000',
 *   peerId: 'peer-bob',
 *   timestamp: '2026-02-01T12:00:00.000Z'
 * };
 * ```
 */
export interface ClaimSettlementInitiatedEvent {
  /** Event type discriminator */
  type: 'CLAIM_SETTLEMENT_INITIATED';
  /** Connector node ID initiating settlement */
  nodeId: string;
  /** Blockchain network ('evm') */
  chain: 'evm';
  /** Channel ID (EVM: bytes32) */
  channelId: string;
  /** Settlement amount, bigint as string */
  amount: string;
  /** Peer identifier */
  peerId: string;
  /** Event timestamp (ISO 8601 format) */
  timestamp: string;
}

/**
 * Claim Settlement Success Telemetry Event
 *
 * Emitted when automatic settlement execution completes successfully on-chain.
 * Indicates settlement transaction confirmed, channel balances updated.
 *
 * **Dashboard Usage:**
 * - Explorer UI shows successful settlement completion
 * - Settlement monitoring panel updates channel status
 *
 * @example
 * ```typescript
 * const event: ClaimSettlementSuccessEvent = {
 *   type: 'CLAIM_SETTLEMENT_SUCCESS',
 *   nodeId: 'connector-a',
 *   chain: 'evm',
 *   channelId: '0xabc123...',
 *   txHash: '0xdef456...',
 *   settledAmount: '5000000000000000000',
 *   peerId: 'peer-bob',
 *   timestamp: '2026-02-01T12:01:00.000Z'
 * };
 * ```
 */
export interface ClaimSettlementSuccessEvent {
  /** Event type discriminator */
  type: 'CLAIM_SETTLEMENT_SUCCESS';
  /** Connector node ID completing settlement */
  nodeId: string;
  /** Blockchain network ('evm') */
  chain: 'evm';
  /** Channel ID */
  channelId: string;
  /** On-chain transaction hash */
  txHash: string;
  /** Settled amount, bigint as string */
  settledAmount: string;
  /** Peer identifier */
  peerId: string;
  /** Event timestamp (ISO 8601 format) */
  timestamp: string;
}

/**
 * Claim Settlement Failed Telemetry Event
 *
 * Emitted when automatic settlement execution fails.
 * Indicates settlement transaction failed, or no claim available for settlement.
 *
 * **Dashboard Usage:**
 * - Explorer UI shows settlement failures for investigation
 * - Settlement monitoring panel displays failed settlements
 *
 * @example
 * ```typescript
 * const event: ClaimSettlementFailedEvent = {
 *   type: 'CLAIM_SETTLEMENT_FAILED',
 *   nodeId: 'connector-a',
 *   chain: 'evm',
 *   channelId: '0xabc123...',
 *   error: 'No stored claim available',
 *   attemptedAmount: '5000000000000000000',
 *   peerId: 'peer-bob',
 *   timestamp: '2026-02-01T12:00:00.000Z'
 * };
 * ```
 */
export interface ClaimSettlementFailedEvent {
  /** Event type discriminator */
  type: 'CLAIM_SETTLEMENT_FAILED';
  /** Connector node ID failing settlement */
  nodeId: string;
  /** Blockchain network ('evm') */
  chain: 'evm';
  /** Channel ID */
  channelId: string;
  /** Error message describing failure */
  error: string;
  /** Attempted settlement amount, bigint as string */
  attemptedAmount: string;
  /** Peer identifier */
  peerId: string;
  /** Event timestamp (ISO 8601 format) */
  timestamp: string;
}

/**
 * Claim Sent Telemetry Event
 *
 * Emitted when ClaimSender (Story 17.2) sends payment channel claim via BTP.
 * Indicates off-chain claim transmitted to peer for redemption.
 *
 * **Dashboard Usage:**
 * - Explorer UI shows claim transmission events
 * - Settlement monitoring panel displays claim send success/failure
 *
 * @example
 * ```typescript
 * const event: ClaimSentEvent = {
 *   type: 'CLAIM_SENT',
 *   nodeId: 'connector-a',
 *   peerId: 'peer-bob',
 *   blockchain: 'xrp',
 *   messageId: 'xrp-a1b2c3d4-n/a-1706889600000',
 *   amount: '1000000',
 *   success: true,
 *   timestamp: '2026-02-02T12:00:00.000Z'
 * };
 * ```
 */
export interface ClaimSentEvent {
  /** Event type discriminator */
  type: 'CLAIM_SENT';
  /** Connector node ID sending claim */
  nodeId: string;
  /** Peer identifier receiving claim */
  peerId: string;
  /** Blockchain type: 'evm' */
  blockchain: string;
  /** Unique message ID for idempotency */
  messageId: string;
  /** Claim amount, bigint as string */
  amount: string;
  /** Whether claim send was successful */
  success: boolean;
  /** Error message if success=false */
  error?: string;
  /** Event timestamp (ISO 8601 format) */
  timestamp: string;
}

/**
 * Claim Received Telemetry Event
 *
 * Emitted when ClaimReceiver (Story 17.3) receives payment channel claim via BTP.
 * Indicates off-chain claim received from peer and verification result.
 *
 * **Dashboard Usage:**
 * - Explorer UI shows claim reception events
 * - Settlement monitoring panel displays claim verification success/failure
 *
 * @example
 * ```typescript
 * const event: ClaimReceivedEvent = {
 *   type: 'CLAIM_RECEIVED',
 *   nodeId: 'connector-a',
 *   peerId: 'peer-bob',
 *   blockchain: 'xrp',
 *   messageId: 'xrp-a1b2c3d4-n/a-1706889600000',
 *   channelId: 'a1b2c3d4e5f6789...',
 *   amount: '1000000',
 *   verified: true,
 *   timestamp: '2026-02-02T12:00:00.000Z'
 * };
 * ```
 */
export interface ClaimReceivedEvent {
  /** Event type discriminator */
  type: 'CLAIM_RECEIVED';
  /** Connector node ID receiving claim */
  nodeId: string;
  /** Peer identifier sending claim */
  peerId: string;
  /** Blockchain type: 'evm' */
  blockchain: string;
  /** Unique message ID for idempotency */
  messageId: string;
  /** Channel ID (EVM: bytes32) */
  channelId: string;
  /** Claim amount, bigint as string */
  amount: string;
  /** Whether claim passed verification */
  verified: boolean;
  /** Error message if verified=false */
  error?: string;
  /** Event timestamp (ISO 8601 format) */
  timestamp: string;
}

/**
 * Claim Redeemed Telemetry Event
 *
 * Emitted when a verified payment channel claim is successfully (or unsuccessfully)
 * redeemed on-chain by the ClaimRedemptionService (Story 17.5).
 *
 * **Emission Points:**
 * - After each claim redemption attempt (success or failure)
 * - In ClaimRedemptionService._emitRedemptionTelemetry()
 *
 * **Use Cases:**
 * - Monitor automatic claim redemption success rate
 * - Track gas costs for redemption profitability analysis
 * - Correlate CLAIM_RECEIVED → CLAIM_REDEEMED for end-to-end claim flow
 * - Detect redemption failures for investigation
 *
 * @example
 * ```typescript
 * const event: ClaimRedeemedEvent = {
 *   type: 'CLAIM_REDEEMED',
 *   nodeId: 'connector-bob',
 *   peerId: 'connector-alice',
 *   blockchain: 'xrp',
 *   messageId: 'msg_xyz123',
 *   channelId: 'ABC123...',
 *   amount: '5000000',
 *   txHash: 'msg_xyz123',
 *   gasCost: '10',
 *   success: true,
 *   timestamp: '2026-02-02T12:00:00.000Z'
 * };
 * ```
 */
export interface ClaimRedeemedEvent {
  /** Event type discriminator */
  type: 'CLAIM_REDEEMED';
  /** Connector node ID redeeming claim */
  nodeId: string;
  /** Peer identifier who sent the claim */
  peerId: string;
  /** Blockchain type: 'evm' */
  blockchain: 'evm';
  /** Unique message ID from CLAIM_RECEIVED (for correlation) */
  messageId: string;
  /** Channel ID (EVM: bytes32) */
  channelId: string;
  /** Claim amount redeemed, bigint as string */
  amount: string;
  /**
   * Transaction hash or identifier.
   * Note: Currently set to messageId since SDK methods return void.
   * For actual blockchain tx hashes, query explorers using signatures/amounts
   * and redeemed_at timestamp.
   */
  txHash: string;
  /** Estimated gas cost for redemption, bigint as string */
  gasCost: string;
  /** Whether redemption succeeded */
  success: boolean;
  /** Error message if success=false */
  error?: string;
  /** Event timestamp (ISO 8601 format) */
  timestamp: string;
}

/**
 * Per-Hop Notification Telemetry Event
 *
 * Emitted when PacketHandler dispatches a fire-and-forget BLS notification
 * at an intermediate hop. Indicates that the per-hop notification pipeline
 * is active and a transit notification was sent to the local BLS.
 *
 * **Dashboard Usage:**
 * - Explorer UI displays per-hop notification events in telemetry tab
 * - Network visualization shows notification activity at each hop
 */
export interface PerHopNotificationEvent {
  /** Event type discriminator */
  type: 'PER_HOP_NOTIFICATION';
  /** Connector node ID emitting event */
  nodeId: string;
  /** ILP destination address */
  destination: string;
  /** Packet amount, bigint as string */
  amount: string;
  /** Next hop peer identifier (the peer the packet is being forwarded to) */
  nextHop: string;
  /** Source peer identifier (the peer the packet was received from) */
  sourcePeer: string;
  /** Correlation ID for packet tracking */
  correlationId: string;
  /** Event timestamp (Unix milliseconds) */
  timestamp: number;
}

/**
 * Telemetry Event Union Type
 *
 * Discriminated union of all telemetry event types.
 * Use `event.type` to narrow to specific event interface.
 *
 * @example
 * ```typescript
 * function handleTelemetryEvent(event: TelemetryEvent): void {
 *   switch (event.type) {
 *     case 'ACCOUNT_BALANCE':
 *       console.log(`Balance updated: ${event.peerId} = ${event.creditBalance}`);
 *       break;
 *     case 'SETTLEMENT_TRIGGERED':
 *       console.log(`Settlement triggered: ${event.peerId}, threshold exceeded by ${event.exceedsBy}`);
 *       break;
 *     case 'SETTLEMENT_COMPLETED':
 *       console.log(`Settlement ${event.success ? 'succeeded' : 'failed'}: ${event.peerId}`);
 *       break;
 *     case 'AGENT_BALANCE_CHANGED':
 *       console.log(`Agent balance changed: ${event.agentId} ${event.token} = ${event.newBalance}`);
 *       break;
 *     case 'AGENT_WALLET_FUNDED':
 *       console.log(`Agent wallet funded: ${event.agentId} with ${event.transactions.length} transactions`);
 *       break;
 *     case 'AGENT_WALLET_STATE_CHANGED':
 *       console.log(`Agent wallet state changed: ${event.agentId} ${event.oldState} → ${event.newState}`);
 *       break;
 *     case 'PAYMENT_CHANNEL_OPENED':
 *       console.log(`Payment channel opened: ${event.channelId} for peer ${event.peerId}`);
 *       break;
 *     case 'PAYMENT_CHANNEL_BALANCE_UPDATE':
 *       console.log(`Payment channel balance updated: ${event.channelId}`);
 *       break;
 *     case 'PAYMENT_CHANNEL_SETTLED':
 *       console.log(`Payment channel settled: ${event.channelId} via ${event.settlementType}`);
 *       break;
 *     case 'XRP_CHANNEL_OPENED':
 *       console.log(`XRP channel opened: ${event.channelId} to ${event.destination}`);
 *       break;
 *     case 'XRP_CHANNEL_CLAIMED':
 *       console.log(`XRP channel claimed: ${event.channelId} amount ${event.claimAmount}`);
 *       break;
 *     case 'XRP_CHANNEL_CLOSED':
 *       console.log(`XRP channel closed: ${event.channelId} via ${event.closeType}`);
 *       break;
 *     case 'WALLET_BALANCE_MISMATCH':
 *       console.log(`Wallet balance mismatch: ${event.agentId} ${event.chain}:${event.token} expected ${event.expectedBalance}, got ${event.actualBalance}`);
 *       break;
 *     case 'SUSPICIOUS_ACTIVITY_DETECTED':
 *       console.log(`Suspicious activity: ${event.agentId} ${event.activityType}`);
 *       break;
 *     case 'RATE_LIMIT_EXCEEDED':
 *       console.log(`Rate limit exceeded: ${event.operation} for ${event.identifier}`);
 *       break;
 *     default:
 *       console.log(`Unknown event type: ${event.type}`);
 *   }
 * }
 * ```
 */
export type TelemetryEvent =
  | PacketReceivedEvent
  | PacketForwardedEvent
  | PacketFulfilledEvent
  | PacketRejectedEvent
  | AccountBalanceEvent
  | SettlementTriggeredEvent
  | SettlementCompletedEvent
  | AgentBalanceChangedEvent
  | AgentWalletFundedEvent
  | FundingRateLimitExceededEvent
  | FundingTransactionConfirmedEvent
  | FundingTransactionFailedEvent
  | AgentWalletStateChangedEvent
  | PaymentChannelOpenedEvent
  | PaymentChannelBalanceUpdateEvent
  | PaymentChannelSettledEvent
  | XRPChannelOpenedEvent
  | XRPChannelClaimedEvent
  | XRPChannelClosedEvent
  | AgentChannelOpenedEvent
  | AgentChannelPaymentSentEvent
  | AgentChannelBalanceUpdateEvent
  | AgentChannelClosedEvent
  | WalletBalanceMismatchEvent
  | SuspiciousActivityDetectedEvent
  | RateLimitExceededEvent
  | ClaimSettlementInitiatedEvent
  | ClaimSettlementSuccessEvent
  | ClaimSettlementFailedEvent
  | ClaimSentEvent
  | ClaimReceivedEvent
  | ClaimRedeemedEvent
  | PerHopNotificationEvent;
