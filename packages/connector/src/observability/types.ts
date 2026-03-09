/**
 * Observability Types - Interfaces for metrics, tracing, and monitoring
 * @packageDocumentation
 * @remarks
 * Defines type interfaces for Prometheus metrics, OpenTelemetry tracing,
 * and SLA monitoring as specified in Story 12.6.
 */

/**
 * Configuration for Prometheus metrics exporter
 */
export interface PrometheusMetricsConfig {
  /** Whether metrics collection is enabled */
  enabled: boolean;
  /** Port for metrics endpoint (default: shares with health server on 8080) */
  port?: number;
  /** Path for metrics endpoint (default: /metrics) */
  metricsPath?: string;
  /** Include Node.js default metrics (default: true) */
  includeDefaultMetrics?: boolean;
  /** Global labels applied to all metrics (e.g., nodeId, environment) */
  labels?: Record<string, string>;
}

/**
 * Configuration for OpenTelemetry distributed tracing
 */
export interface OpenTelemetryConfig {
  /** Whether tracing is enabled */
  enabled: boolean;
  /** Service name for traces (e.g., 'agent-runtime') */
  serviceName: string;
  /** OTLP exporter endpoint (default: http://localhost:4318) */
  exporterEndpoint?: string;
  /** Sampling ratio 0.0-1.0 (default: 1.0 for 100% sampling) */
  samplingRatio?: number;
}

/**
 * Configuration for SLA monitoring thresholds
 */
export interface SLAConfig {
  /** Packet success rate threshold (default: 0.999 = 99.9%) */
  packetSuccessRateThreshold: number;
  /** Settlement success rate threshold (default: 0.99 = 99%) */
  settlementSuccessRateThreshold: number;
  /** P99 latency threshold in milliseconds (default: 10) */
  p99LatencyThresholdMs: number;
}

/**
 * Combined observability configuration
 */
export interface ObservabilityConfig {
  prometheus: PrometheusMetricsConfig;
  opentelemetry?: OpenTelemetryConfig;
  sla?: SLAConfig;
}

/**
 * ILP packet type for metrics labeling
 */
export type ILPPacketType = 'prepare' | 'fulfill' | 'reject';

/**
 * Packet processing status for metrics labeling
 */
export type PacketStatus = 'success' | 'error' | 'timeout' | 'rejected';

/**
 * Settlement method for metrics labeling
 */
export type SettlementMethod = 'evm' | 'tigerbeetle';

/**
 * Settlement operation status
 */
export type SettlementStatus = 'success' | 'failure' | 'pending';

/**
 * Payment channel status
 */
export type ChannelStatus = 'open' | 'funded' | 'closing' | 'closed' | 'disputed';

/**
 * Channel lifecycle event type
 */
export type ChannelEvent = 'funded' | 'closed' | 'disputed';

/**
 * Error severity level
 */
export type ErrorSeverity = 'low' | 'medium' | 'high' | 'critical';

/**
 * Error type classification
 */
export type ErrorType =
  | 'packet_validation'
  | 'routing'
  | 'settlement'
  | 'channel'
  | 'connection'
  | 'internal';

/**
 * Prometheus Alert Rule definition
 */
export interface AlertRule {
  /** Alert name */
  name: string;
  /** PromQL expression */
  expr: string;
  /** Duration before firing (e.g., '2m') */
  for: string;
  /** Alert severity */
  severity: 'warning' | 'critical' | 'high';
  /** Short alert summary */
  summary: string;
  /** Detailed alert description */
  description: string;
}

/**
 * Dependency health status
 */
export interface DependencyStatus {
  /** Whether the dependency is reachable */
  status: 'up' | 'down';
  /** Latency of health check in milliseconds */
  latencyMs?: number;
}

/**
 * SLA metrics snapshot
 */
export interface SLAMetrics {
  /** Packet delivery success rate (0.0 - 1.0) */
  packetSuccessRate: number;
  /** Settlement success rate (0.0 - 1.0) */
  settlementSuccessRate: number;
  /** P99 latency in milliseconds */
  p99LatencyMs: number;
}

/**
 * Metrics recording options for packet processing
 */
export interface PacketMetricsOptions {
  type: ILPPacketType;
  status: PacketStatus;
  latencyMs: number;
  destination?: string;
  peerId?: string;
}

/**
 * Metrics recording options for settlement operations
 */
export interface SettlementMetricsOptions {
  method: SettlementMethod;
  status: SettlementStatus;
  latencyMs: number;
  amount?: bigint;
  tokenId?: string;
  peerId?: string;
}

/**
 * Metrics recording options for channel events
 */
export interface ChannelMetricsOptions {
  method: SettlementMethod;
  event: ChannelEvent;
  reason?: string;
  channelId?: string;
}

/**
 * Metrics recording options for errors
 */
export interface ErrorMetricsOptions {
  type: ErrorType;
  severity: ErrorSeverity;
  message?: string;
}

/**
 * Blockchain type for claim metrics
 */
export type ClaimBlockchain = 'evm';

/**
 * Metrics recording options for claim operations
 */
export interface ClaimMetricsOptions {
  blockchain: ClaimBlockchain;
  peerId: string;
  success?: boolean;
  verified?: boolean;
  errorType?: string;
  latencyMs?: number;
}
