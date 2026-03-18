/**
 * ILP Connector Library Exports
 * Side-effect-free entry point for library consumers
 * @packageDocumentation
 */

import { ConnectorNode } from './core/connector-node';
import { ConfigLoader, ConfigurationError, ConnectorNotStartedError } from './config/config-loader';
import { createLogger } from './utils/logger';
import { RoutingTable } from './routing/routing-table';
import { PacketHandler } from './core/packet-handler';
import { BTPServer } from './btp/btp-server';
import { BTPClient } from './btp/btp-client';
import { BTPClientManager } from './btp/btp-client-manager';
// LocalDeliveryClient is INTERNAL ONLY - not exported
// Library consumers should use ConnectorNode.setLocalDeliveryHandler() instead
// import { LocalDeliveryClient } from './core/local-delivery-client';
import { AdminServer } from './http/admin-server';
import { AccountManager } from './settlement/account-manager';
import { SettlementMonitor } from './settlement/settlement-monitor';
import { UnifiedSettlementExecutor } from './settlement/unified-settlement-executor';
import {
  createPaymentHandlerAdapter,
  REJECT_CODE_MAP,
  computeFulfillmentFromData,
  generatePaymentId,
  mapRejectCode,
  validateResponseData,
} from './core/payment-handler';
import {
  IlpSendHandler,
  computeConditionFromData,
  validateIlpSendRequest,
} from './http/ilp-send-handler';

// Export public API
export {
  ConnectorNode,
  ConfigLoader,
  ConfigurationError,
  ConnectorNotStartedError,
  RoutingTable,
  PacketHandler,
  BTPServer,
  BTPClient,
  BTPClientManager,
  // LocalDeliveryClient is INTERNAL ONLY - not exported
  // Library consumers should use ConnectorNode.setLocalDeliveryHandler() instead
  AdminServer,
  AccountManager,
  SettlementMonitor,
  UnifiedSettlementExecutor,
  createLogger,
  // Payment handler utilities
  createPaymentHandlerAdapter,
  REJECT_CODE_MAP,
  computeFulfillmentFromData,
  generatePaymentId,
  mapRejectCode,
  validateResponseData,
  // ILP send handler
  IlpSendHandler,
  computeConditionFromData,
  validateIlpSendRequest,
};

// Export configuration types
export type {
  ConnectorConfig,
  PeerConfig,
  RouteConfig,
  SettlementConfig,
  LocalDeliveryConfig,
  LocalDeliveryHandler,
  LocalDeliveryRequest,
  LocalDeliveryResponse,
  SendPacketParams,
  SettlementInfraConfig,
  PeerRegistrationRequest,
  PeerInfo,
  PeerAccountBalance,
  RouteInfo,
  RemovePeerResult,
  IlpSendRequest,
  IlpSendResponse,
} from './config/types';

// Re-export settlement types for library consumers
export type { AdminSettlementConfig } from './settlement/types';

// Re-export channel manager types for library consumers (embedded mode)
export type { ChannelOpenOptions, ChannelMetadata } from './settlement/channel-manager';

// Re-export payment handler types for library consumers
export type { PaymentRequest, PaymentResponse, PaymentHandler } from './core/payment-handler';

// Re-export ILP send handler types for library consumers
export type { PacketSenderFn, IsReadyFn } from './http/ilp-send-handler';

// Re-export ILP packet types for library consumers
export type { ILPPreparePacket, ILPFulfillPacket, ILPRejectPacket } from '@toon-protocol/shared';
