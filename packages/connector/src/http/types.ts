/**
 * Health status response interface for health check endpoint
 * @description Represents the current operational status of a connector node
 */
export interface HealthStatus {
  /**
   * Overall health state of the connector
   * - 'healthy': All critical systems operational, ≥50% peers connected
   * - 'unhealthy': <50% peers connected, indicating network partition or peer failures
   * - 'starting': Connector initialization in progress, BTP server not yet listening
   */
  status: 'healthy' | 'unhealthy' | 'starting';

  /**
   * Seconds since connector started
   * Calculated as: Math.floor((Date.now() - startTime) / 1000)
   */
  uptime: number;

  /**
   * Number of peers currently connected via BTP
   * Obtained from BTPClientManager.getConnectedPeerCount()
   */
  peersConnected: number;

  /**
   * Total number of configured peers
   * Obtained from config.peers.length
   */
  totalPeers: number;

  /**
   * ISO 8601 timestamp of when this health check was performed
   * Example: "2025-12-27T10:30:00.000Z"
   */
  timestamp: string;

  /**
   * Optional connector node identifier from configuration
   * Useful for debugging in multi-node deployments
   */
  nodeId?: string;

  /**
   * Optional connector version from package.json
   * Example: "1.0.0"
   */
  version?: string;

  /**
   * Optional explorer status when explorer UI is enabled
   * Present only when config.explorer.enabled is true (or not explicitly false)
   */
  explorer?: {
    /** Whether explorer is enabled */
    enabled: boolean;
    /** Explorer server port */
    port: number;
    /** Number of events stored in EventStore */
    eventCount: number;
    /** Number of active WebSocket connections */
    wsConnections: number;
  };
}

/**
 * Interface for components that can provide health status
 * @description Implemented by ConnectorNode to supply health data to HealthServer
 * @example
 * class ConnectorNode implements HealthStatusProvider {
 *   getHealthStatus(): HealthStatus {
 *     return {
 *       status: this.calculateStatus(),
 *       uptime: Math.floor((Date.now() - this._startTime.getTime()) / 1000),
 *       peersConnected: this._btpClientManager.getConnectedPeerCount(),
 *       totalPeers: this._config.peers.length,
 *       timestamp: new Date().toISOString()
 *     };
 *   }
 * }
 */
export interface HealthStatusProvider {
  /**
   * Returns the current health status of the connector
   * @returns HealthStatus object with all current health metrics
   */
  getHealthStatus(): HealthStatus;
}

/**
 * Dependency health status for external services
 * @description Status and latency of dependency health checks
 */
export interface DependencyHealthStatus {
  /** Whether the dependency is reachable */
  status: 'up' | 'down';
  /** Latency of health check in milliseconds */
  latencyMs?: number;
}

/**
 * SLA metrics snapshot for health response
 * @description Current SLA metric values for monitoring
 */
export interface SLAMetricsSnapshot {
  /** Packet delivery success rate (0.0 - 1.0) */
  packetSuccessRate: number;
  /** Settlement success rate (0.0 - 1.0) */
  settlementSuccessRate: number;
  /** P99 latency in milliseconds */
  p99LatencyMs: number;
}

/**
 * Extended health status with dependency checks and SLA metrics
 * @description Production-ready health status for Kubernetes probes and monitoring
 * Extends base HealthStatus with additional production monitoring fields
 */
export interface HealthStatusExtended extends Omit<HealthStatus, 'status'> {
  /**
   * Overall health state of the connector (extended)
   * - 'healthy': All critical systems operational, all dependencies up
   * - 'degraded': Some non-critical dependencies down or SLA thresholds breached
   * - 'unhealthy': Critical dependencies down or major failures
   * - 'starting': Connector initialization in progress
   */
  status: 'healthy' | 'degraded' | 'unhealthy' | 'starting';

  /**
   * Dependency health status
   * Includes TigerBeetle and EVM health checks
   */
  dependencies: {
    tigerbeetle: DependencyHealthStatus;
    evm?: DependencyHealthStatus;
  };

  /**
   * Current SLA metrics
   * Used to determine degraded status if thresholds are breached
   */
  sla: SLAMetricsSnapshot;
}

/**
 * Interface for components that can provide extended health status
 * @description Extended provider with dependency and SLA health checks
 */
export interface HealthStatusExtendedProvider extends HealthStatusProvider {
  /**
   * Returns extended health status with dependency and SLA information
   * @returns HealthStatusExtended object with all health metrics
   */
  getHealthStatusExtended(): HealthStatusExtended;
}
