/**
 * Peer Discovery Type Definitions
 *
 * Types for peer discovery and automatic peer connection.
 */

/**
 * Configuration for the peer discovery service
 */
export interface PeerDiscoveryConfig {
  /** Whether peer discovery is enabled */
  enabled: boolean;

  /** Seconds between broadcast announcements (default: 60) */
  broadcastInterval: number;

  /** URLs of discovery service endpoints */
  discoveryEndpoints?: string[];

  /** Public address to announce (auto-detected if not set) */
  announceAddress?: string;

  /** Local node ID */
  nodeId: string;

  /** Local BTP endpoint URL */
  btpEndpoint: string;

  /** Local ILP address prefix */
  ilpAddress: string;

  /** Supported settlement capabilities */
  capabilities: string[];

  /** Connector version */
  version: string;
}

/**
 * Information about a discovered peer
 */
export interface PeerInfo {
  /** Unique connector identifier */
  nodeId: string;

  /** WebSocket URL for BTP connection (e.g., "ws://host:4000") */
  btpEndpoint: string;

  /** ILP address prefix (e.g., "g.connector.alice") */
  ilpAddress: string;

  /** Supported features: ['evm-settlement'] */
  capabilities: string[];

  /** Unix timestamp of last heartbeat */
  lastSeen: number;

  /** Connector version */
  version: string;
}

/**
 * Response from peer announcement endpoint
 */
export interface AnnounceResponse {
  /** Whether the announcement was successful */
  success: boolean;

  /** TTL in seconds before re-announce needed */
  ttl?: number;

  /** Error message if unsuccessful */
  error?: string;
}

/**
 * Response from peer list endpoint
 */
export interface PeerListResponse {
  /** List of active peers */
  peers: PeerInfo[];

  /** Total number of peers */
  total: number;
}

/**
 * Discovery service status
 */
export type DiscoveryStatus = 'stopped' | 'starting' | 'running' | 'stopping';

/**
 * Events emitted by the peer discovery service
 */
export interface PeerDiscoveryEvents {
  /** Emitted when a new peer is discovered */
  'peer:discovered': (peer: PeerInfo) => void;

  /** Emitted when a peer is removed (TTL expired) */
  'peer:removed': (nodeId: string) => void;

  /** Emitted when connected to a peer */
  'peer:connected': (peer: PeerInfo) => void;

  /** Emitted when connection to a peer fails */
  'peer:connection-failed': (peer: PeerInfo, error: Error) => void;

  /** Emitted when discovery service status changes */
  'status:changed': (status: DiscoveryStatus) => void;

  /** Emitted when an error occurs */
  error: (error: Error) => void;
}
