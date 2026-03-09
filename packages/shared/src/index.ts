/**
 * Shared types and utilities
 * @packageDocumentation
 */

export const version = '1.0.0';

// ILP Type Definitions (RFC-0027, RFC-0015)
export {
  // Enums
  PacketType,
  ILPErrorCode,
  // Types
  ILPAddress,
  ILPPacket,
  ILPPreparePacket,
  ILPFulfillPacket,
  ILPRejectPacket,
  // Type Guards
  isPreparePacket,
  isFulfillPacket,
  isRejectPacket,
  // Validation Helpers
  isValidILPAddress,
} from './types/ilp';

// OER Encoding/Decoding (RFC-0030)
export {
  // Error Classes
  InvalidPacketError,
  BufferUnderflowError,
  // Generic Packet Serialization
  serializePacket,
  deserializePacket,
  // Type-Specific Serialization
  serializePrepare,
  deserializePrepare,
  serializeFulfill,
  deserializeFulfill,
  serializeReject,
  deserializeReject,
  // OER Primitives
  encodeVarUInt,
  decodeVarUInt,
  encodeVarOctetString,
  decodeVarOctetString,
  encodeFixedOctetString,
  decodeFixedOctetString,
  encodeGeneralizedTime,
  decodeGeneralizedTime,
} from './encoding/oer';

// Routing Types
export { RoutingTableEntry } from './types/routing';

// Telemetry Types (Story 6.8, Story 11.3, Story 11.4, Story 11.5, Story 8.10, Story 9.7, Story 11.6, Story 17.2, Story 17.3)
export {
  TelemetryEventType,
  SettlementState,
  AccountBalanceEvent,
  SettlementTriggeredEvent,
  SettlementCompletedEvent,
  AgentBalanceChangedEvent,
  FundingTransaction,
  AgentWalletFundedEvent,
  FundingRateLimitExceededEvent,
  FundingTransactionConfirmedEvent,
  FundingTransactionFailedEvent,
  AgentWalletStateChangedEvent,
  AgentChannelOpenedEvent,
  AgentChannelPaymentSentEvent,
  AgentChannelClosedEvent,
  ClaimSentEvent,
  ClaimReceivedEvent,
  PerHopNotificationEvent,
  TelemetryEvent,
} from './types/telemetry';

// Payment Channel Telemetry Types (Epic 8 Story 8.10, Epic 27 Story 27.5)
export {
  PaymentChannelOpenedEvent,
  PaymentChannelBalanceUpdateEvent,
  PaymentChannelSettledEvent,
  DashboardChannelState,
} from './types/payment-channel-telemetry';

// Payment Channel Types (Epic 8 Story 8.7)
export {
  ChannelStatus,
  ChannelState,
  BalanceProof,
  ChannelOpenedEvent,
  ChannelClosedEvent,
  ChannelSettledEvent,
  ChannelCooperativeSettledEvent,
  ChannelEvent,
} from './types/payment-channel';
