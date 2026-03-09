import { EventEmitter } from 'events';
import { Logger } from 'pino';

/**
 * Base interface for all fraud detection rules
 */
export interface FraudRule {
  name: string;
  check(event: SettlementEvent | PacketEvent | ChannelEvent): Promise<FraudDetection>;
  severity: 'low' | 'medium' | 'high' | 'critical';
}

/**
 * Result of fraud rule evaluation
 */
export interface FraudDetection {
  detected: boolean;
  peerId?: string;
  details?: {
    [key: string]: unknown;
    description?: string;
  };
}

/**
 * Settlement event data structure
 */
export interface SettlementEvent {
  type: 'settlement';
  peerId: string;
  amount: number;
  timestamp: number;
  channelId?: string;
}

/**
 * Packet event data structure
 */
export interface PacketEvent {
  type: 'packet';
  peerId: string;
  packetCount: number;
  timestamp: number;
}

/**
 * Channel lifecycle event data structure
 */
export interface ChannelEvent {
  type: 'channel';
  peerId: string;
  action: 'open' | 'close';
  channelId: string;
  timestamp: number;
}

/**
 * Peer reputation score with violation history
 */
export interface PeerReputationScore {
  peerId: string;
  score: number; // 0-100, initialized at 100 (perfect)
  lastUpdated: number; // Timestamp
  violations: {
    timestamp: number;
    ruleViolated: string;
    severity: 'low' | 'medium' | 'high' | 'critical';
    penaltyApplied: number;
  }[];
}

/**
 * Pause reason tracking
 */
export interface PauseReason {
  peerId: string;
  reason: string;
  timestamp: number;
  ruleViolated: string;
  severity: 'low' | 'medium' | 'high' | 'critical';
}

/**
 * FraudDetector configuration
 */
export interface FraudDetectorConfig {
  enabled: boolean;
  autoPauseThreshold: number; // Score threshold for auto-pause (default: 50)
  rules: FraudRule[];
}

/**
 * FraudDetector service for detecting and responding to suspicious activity
 *
 * Monitors settlement patterns, packet volumes, and channel behaviors to detect
 * fraud attempts. Maintains peer reputation scores and can auto-pause suspicious peers.
 */
export class FraudDetector extends EventEmitter {
  private readonly logger: Logger;
  private readonly config: FraudDetectorConfig;
  private readonly rules: FraudRule[];
  private readonly pausedPeers: Map<string, PauseReason>;

  // Store bound handler references for proper event listener cleanup
  private readonly boundHandlers: {
    handleSettlementEvent: (event: SettlementEvent) => Promise<void>;
    handlePacketEvent: (event: PacketEvent) => Promise<void>;
    handleChannelEvent: (event: ChannelEvent) => Promise<void>;
  };

  constructor(logger: Logger, config: FraudDetectorConfig) {
    super();
    this.logger = logger.child({ component: 'FraudDetector' });
    this.config = config;
    this.rules = config.rules;
    this.pausedPeers = new Map();

    // Store bound handler references for proper cleanup
    this.boundHandlers = {
      handleSettlementEvent: this.handleSettlementEvent.bind(this),
      handlePacketEvent: this.handlePacketEvent.bind(this),
      handleChannelEvent: this.handleChannelEvent.bind(this),
    };

    this.logger.info('FraudDetector initialized', {
      enabled: config.enabled,
      ruleCount: this.rules.length,
      autoPauseThreshold: config.autoPauseThreshold,
    });
  }

  /**
   * Start fraud detection monitoring
   */
  public start(): void {
    if (!this.config.enabled) {
      this.logger.info('FraudDetector is disabled');
      return;
    }

    // Register event listeners using stored bound handlers
    this.on('SETTLEMENT_EVENT', this.boundHandlers.handleSettlementEvent);
    this.on('PACKET_EVENT', this.boundHandlers.handlePacketEvent);
    this.on('CHANNEL_EVENT', this.boundHandlers.handleChannelEvent);

    this.logger.info('FraudDetector started');
  }

  /**
   * Stop fraud detection monitoring and cleanup event listeners
   */
  public stop(): void {
    // Remove event listeners using stored bound handlers
    this.off('SETTLEMENT_EVENT', this.boundHandlers.handleSettlementEvent);
    this.off('PACKET_EVENT', this.boundHandlers.handlePacketEvent);
    this.off('CHANNEL_EVENT', this.boundHandlers.handleChannelEvent);

    this.logger.info('FraudDetector stopped');
  }

  /**
   * Handle settlement events from SettlementMonitor
   */
  private async handleSettlementEvent(event: SettlementEvent): Promise<void> {
    await this.analyzeEvent(event);
  }

  /**
   * Handle packet events from PacketHandler
   */
  private async handlePacketEvent(event: PacketEvent): Promise<void> {
    await this.analyzeEvent(event);
  }

  /**
   * Handle channel lifecycle events
   */
  private async handleChannelEvent(event: ChannelEvent): Promise<void> {
    await this.analyzeEvent(event);
  }

  /**
   * Analyze event against all fraud detection rules
   */
  public async analyzeEvent(event: SettlementEvent | PacketEvent | ChannelEvent): Promise<void> {
    if (!this.config.enabled) {
      return;
    }

    // Check if peer is already paused
    if (event.peerId && this.pausedPeers.has(event.peerId)) {
      this.logger.debug('Event from paused peer ignored', { peerId: event.peerId });
      return;
    }

    // Evaluate event against all rules
    for (const rule of this.rules) {
      try {
        const detection = await rule.check(event);
        if (detection.detected) {
          await this.handleFraudDetection(rule, detection, event);
        }
      } catch (error) {
        // Rule evaluation failure: log error, continue with remaining rules
        this.logger.error('Fraud rule evaluation failed', {
          ruleName: rule.name,
          error: error instanceof Error ? error.message : String(error),
        });
      }
    }
  }

  /**
   * Handle fraud detection result
   */
  private async handleFraudDetection(
    rule: FraudRule,
    detection: FraudDetection,
    event: SettlementEvent | PacketEvent | ChannelEvent
  ): Promise<void> {
    const peerId = detection.peerId || event.peerId;

    this.logger.warn('Fraud detected', {
      ruleName: rule.name,
      severity: rule.severity,
      peerId,
      details: detection.details,
    });

    // Emit fraud detection event for external handling (metrics, alerts, reputation)
    this.emit('FRAUD_DETECTED', {
      ruleName: rule.name,
      severity: rule.severity,
      peerId,
      timestamp: Date.now(),
      details: detection.details,
    });
  }

  /**
   * Pause peer operations due to fraud detection
   *
   * @param peerId - Peer identifier
   * @param reason - Human-readable reason for pause
   * @param ruleViolated - Name of rule that triggered pause
   * @param severity - Severity level of violation
   */
  public async pausePeer(
    peerId: string,
    reason: string,
    ruleViolated: string,
    severity: 'low' | 'medium' | 'high' | 'critical'
  ): Promise<void> {
    try {
      const pauseReason: PauseReason = {
        peerId,
        reason,
        timestamp: Date.now(),
        ruleViolated,
        severity,
      };

      this.pausedPeers.set(peerId, pauseReason);

      this.logger.warn('Peer paused due to fraud detection', {
        peerId,
        reason,
        ruleViolated,
        severity,
      });

      // Emit peer paused event for integration with PacketHandler, SettlementMonitor, etc.
      this.emit('PEER_PAUSED', { peerId, reason, ruleViolated, severity });
    } catch (error) {
      // Pause operation failure: log critical error, alert operators
      this.logger.error('Failed to pause peer', {
        peerId,
        error: error instanceof Error ? error.message : String(error),
      });
      throw error;
    }
  }

  /**
   * Resume peer operations after manual review
   *
   * @param peerId - Peer identifier
   */
  public async resumePeer(peerId: string): Promise<void> {
    try {
      if (!this.pausedPeers.has(peerId)) {
        this.logger.warn('Attempted to resume peer that is not paused', { peerId });
        return;
      }

      this.pausedPeers.delete(peerId);

      this.logger.info('Peer resumed after manual review', { peerId });

      // Emit peer resumed event for integration
      this.emit('PEER_RESUMED', { peerId });
    } catch (error) {
      // Resume operation failure: log critical error, require manual intervention
      this.logger.error('Failed to resume peer', {
        peerId,
        error: error instanceof Error ? error.message : String(error),
      });
      throw error;
    }
  }

  /**
   * Check if peer is currently paused
   */
  public isPeerPaused(peerId: string): boolean {
    return this.pausedPeers.has(peerId);
  }

  /**
   * Get pause reason for peer
   */
  public getPauseReason(peerId: string): PauseReason | undefined {
    return this.pausedPeers.get(peerId);
  }

  /**
   * Get all paused peers
   */
  public getPausedPeers(): Map<string, PauseReason> {
    return new Map(this.pausedPeers);
  }
}
