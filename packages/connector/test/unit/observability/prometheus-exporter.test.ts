/**
 * PrometheusExporter Unit Tests
 * @remarks
 * Tests for Prometheus metrics collection and export functionality.
 * Validates metric registration, recording, and Prometheus format export.
 */

import { Logger } from 'pino';
import { Request, Response } from 'express';
import { PrometheusExporter } from '../../../src/observability/prometheus-exporter';
import {
  PrometheusMetricsConfig,
  PacketMetricsOptions,
  SettlementMetricsOptions,
  ChannelMetricsOptions,
  ErrorMetricsOptions,
  ClaimMetricsOptions,
} from '../../../src/observability/types';

describe('PrometheusExporter', () => {
  let mockLogger: Logger;
  let exporter: PrometheusExporter;

  beforeEach(() => {
    // Create mock logger
    mockLogger = {
      child: jest.fn().mockReturnThis(),
      info: jest.fn(),
      debug: jest.fn(),
      trace: jest.fn(),
      warn: jest.fn(),
      error: jest.fn(),
    } as unknown as Logger;
  });

  afterEach(() => {
    // Clean up exporter registry to prevent metric registration conflicts
    if (exporter) {
      exporter.shutdown();
    }
  });

  describe('constructor', () => {
    it('should initialize with default configuration', async () => {
      exporter = await PrometheusExporter.create(mockLogger);

      expect(mockLogger.child).toHaveBeenCalledWith({ component: 'prometheus-exporter' });
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({ config: expect.any(Object) }),
        'PrometheusExporter initialized'
      );
    });

    it('should accept custom configuration', async () => {
      const config: Partial<PrometheusMetricsConfig> = {
        enabled: true,
        metricsPath: '/custom-metrics',
        includeDefaultMetrics: false,
        labels: { environment: 'test', nodeId: 'node-1' },
      };

      exporter = await PrometheusExporter.create(mockLogger, config);

      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          config: expect.objectContaining({
            metricsPath: '/custom-metrics',
            includeDefaultMetrics: false,
          }),
        }),
        'PrometheusExporter initialized'
      );
    });

    it('should create a custom registry', async () => {
      exporter = await PrometheusExporter.create(mockLogger);
      const registry = exporter.getRegistry();

      expect(registry).toBeDefined();
      expect(typeof registry.metrics).toBe('function');
    });
  });

  describe('recordPacket', () => {
    beforeEach(async () => {
      exporter = await PrometheusExporter.create(mockLogger, { includeDefaultMetrics: false });
    });

    it('should record successful packet', async () => {
      const options: PacketMetricsOptions = {
        type: 'prepare',
        status: 'success',
        latencyMs: 5,
      };

      exporter.recordPacket(options);

      const metrics = await exporter.getMetrics();
      expect(metrics).toContain('ilp_packets_processed_total');
      expect(metrics).toContain('type="prepare"');
      expect(metrics).toContain('status="success"');
    });

    it('should record failed packet', async () => {
      const options: PacketMetricsOptions = {
        type: 'prepare',
        status: 'error',
        latencyMs: 10,
      };

      exporter.recordPacket(options);

      const metrics = await exporter.getMetrics();
      expect(metrics).toContain('status="error"');
    });

    it('should record packet latency histogram', async () => {
      exporter.recordPacket({
        type: 'fulfill',
        status: 'success',
        latencyMs: 50,
      });

      const metrics = await exporter.getMetrics();
      expect(metrics).toContain('ilp_packet_latency_seconds');
      expect(metrics).toContain('type="fulfill"');
    });

    it('should track SLA metrics for packets', () => {
      exporter.recordPacket({ type: 'prepare', status: 'success', latencyMs: 5 });
      exporter.recordPacket({ type: 'prepare', status: 'success', latencyMs: 10 });
      exporter.recordPacket({ type: 'prepare', status: 'error', latencyMs: 15 });

      const slaMetrics = exporter.getSLAMetrics();

      expect(slaMetrics.packetSuccessRate).toBeCloseTo(2 / 3, 2);
    });

    it('should limit latency samples to prevent memory bloat', () => {
      // Record more than 10000 packets
      for (let i = 0; i < 10050; i++) {
        exporter.recordPacket({
          type: 'prepare',
          status: 'success',
          latencyMs: i,
        });
      }

      const slaMetrics = exporter.getSLAMetrics();
      // p99 should be based on last 10000 samples
      expect(slaMetrics.p99LatencyMs).toBeDefined();
    });
  });

  describe('packets in flight', () => {
    beforeEach(async () => {
      exporter = await PrometheusExporter.create(mockLogger, { includeDefaultMetrics: false });
    });

    it('should increment packets in flight', async () => {
      exporter.incrementPacketsInFlight();
      exporter.incrementPacketsInFlight();

      const metrics = await exporter.getMetrics();
      expect(metrics).toContain('ilp_packets_in_flight 2');
    });

    it('should decrement packets in flight', async () => {
      exporter.incrementPacketsInFlight();
      exporter.incrementPacketsInFlight();
      exporter.decrementPacketsInFlight();

      const metrics = await exporter.getMetrics();
      expect(metrics).toContain('ilp_packets_in_flight 1');
    });
  });

  describe('recordSettlement', () => {
    beforeEach(async () => {
      exporter = await PrometheusExporter.create(mockLogger, { includeDefaultMetrics: false });
    });

    it('should record successful settlement', async () => {
      const options: SettlementMetricsOptions = {
        method: 'evm',
        status: 'success',
        latencyMs: 3000,
        amount: BigInt(1000000),
        tokenId: 'AGENT',
      };

      exporter.recordSettlement(options);

      const metrics = await exporter.getMetrics();
      expect(metrics).toContain('settlements_executed_total');
      expect(metrics).toContain('method="evm"');
      expect(metrics).toContain('status="success"');
    });

    it('should record settlement latency', async () => {
      exporter.recordSettlement({
        method: 'evm',
        status: 'success',
        latencyMs: 5000,
      });

      const metrics = await exporter.getMetrics();
      expect(metrics).toContain('settlement_latency_seconds');
      expect(metrics).toContain('method="evm"');
    });

    it('should record settlement amount', async () => {
      exporter.recordSettlement({
        method: 'evm',
        status: 'success',
        latencyMs: 2000,
        amount: BigInt(5000000),
        tokenId: 'AGENT',
      });

      const metrics = await exporter.getMetrics();
      expect(metrics).toContain('settlement_amount_total');
      expect(metrics).toContain('token="AGENT"');
    });

    it('should track SLA metrics for settlements', () => {
      exporter.recordSettlement({ method: 'evm', status: 'success', latencyMs: 1000 });
      exporter.recordSettlement({ method: 'evm', status: 'success', latencyMs: 2000 });
      exporter.recordSettlement({ method: 'evm', status: 'failure', latencyMs: 3000 });

      const slaMetrics = exporter.getSLAMetrics();

      expect(slaMetrics.settlementSuccessRate).toBeCloseTo(2 / 3, 2);
    });
  });

  describe('account metrics', () => {
    beforeEach(async () => {
      exporter = await PrometheusExporter.create(mockLogger, { includeDefaultMetrics: false });
    });

    it('should update account balance', async () => {
      exporter.updateAccountBalance('peer-1', 'XRP', BigInt(1000000));

      const metrics = await exporter.getMetrics();
      expect(metrics).toContain('account_balance_units');
      expect(metrics).toContain('peer_id="peer-1"');
      expect(metrics).toContain('token_id="XRP"');
    });

    it('should record account credits', async () => {
      exporter.recordAccountCredit('peer-1', BigInt(500000));

      const metrics = await exporter.getMetrics();
      expect(metrics).toContain('account_credits_total');
    });

    it('should record account debits', async () => {
      exporter.recordAccountDebit('peer-1', BigInt(250000));

      const metrics = await exporter.getMetrics();
      expect(metrics).toContain('account_debits_total');
    });
  });

  describe('channel metrics', () => {
    beforeEach(async () => {
      exporter = await PrometheusExporter.create(mockLogger, { includeDefaultMetrics: false });
    });

    it('should update active channels count', async () => {
      exporter.updateActiveChannels('evm', 'open', 5);

      const metrics = await exporter.getMetrics();
      expect(metrics).toContain('payment_channels_active');
      expect(metrics).toContain('method="evm"');
      expect(metrics).toContain('status="open"');
    });

    it('should record channel funded event', async () => {
      const options: ChannelMetricsOptions = {
        method: 'evm',
        event: 'funded',
      };

      exporter.recordChannelEvent(options);

      const metrics = await exporter.getMetrics();
      expect(metrics).toContain('payment_channels_funded_total');
    });

    it('should record channel closed event with reason', async () => {
      const options: ChannelMetricsOptions = {
        method: 'evm',
        event: 'closed',
        reason: 'expired',
      };

      exporter.recordChannelEvent(options);

      const metrics = await exporter.getMetrics();
      expect(metrics).toContain('payment_channels_closed_total');
      expect(metrics).toContain('reason="expired"');
    });

    it('should record channel dispute event', async () => {
      const options: ChannelMetricsOptions = {
        method: 'evm',
        event: 'disputed',
      };

      exporter.recordChannelEvent(options);

      const metrics = await exporter.getMetrics();
      expect(metrics).toContain('payment_channels_disputes_total');
    });
  });

  describe('error metrics', () => {
    beforeEach(async () => {
      exporter = await PrometheusExporter.create(mockLogger, { includeDefaultMetrics: false });
    });

    it('should record error', async () => {
      const options: ErrorMetricsOptions = {
        type: 'settlement',
        severity: 'high',
      };

      exporter.recordError(options);

      const metrics = await exporter.getMetrics();
      expect(metrics).toContain('connector_errors_total');
      expect(metrics).toContain('type="settlement"');
      expect(metrics).toContain('severity="high"');
    });

    it('should update last error timestamp', async () => {
      const beforeRecord = Date.now() / 1000;

      exporter.recordError({
        type: 'connection',
        severity: 'critical',
      });

      const metrics = await exporter.getMetrics();
      expect(metrics).toContain('connector_last_error_timestamp');

      // The timestamp should be recent
      const match = metrics.match(/connector_last_error_timestamp\s+(\d+\.?\d*)/);
      expect(match).toBeDefined();
      expect(match).not.toBeNull();
      const timestamp = parseFloat(match![1] as string);
      expect(timestamp).toBeGreaterThanOrEqual(beforeRecord);
    });
  });

  describe('SLA metrics', () => {
    beforeEach(async () => {
      exporter = await PrometheusExporter.create(mockLogger, { includeDefaultMetrics: false });
    });

    it('should return perfect SLA metrics when no data', () => {
      const slaMetrics = exporter.getSLAMetrics();

      expect(slaMetrics.packetSuccessRate).toBe(1.0);
      expect(slaMetrics.settlementSuccessRate).toBe(1.0);
      expect(slaMetrics.p99LatencyMs).toBe(0);
    });

    it('should calculate correct p99 latency', () => {
      // Record 100 packets with increasing latency
      for (let i = 1; i <= 100; i++) {
        exporter.recordPacket({
          type: 'prepare',
          status: 'success',
          latencyMs: i,
        });
      }

      const slaMetrics = exporter.getSLAMetrics();

      // p99 of 1-100: index = floor(100 * 0.99) = 99, value at index 99 is 100
      expect(slaMetrics.p99LatencyMs).toBe(100);
    });
  });

  describe('getMetrics', () => {
    beforeEach(async () => {
      exporter = await PrometheusExporter.create(mockLogger, { includeDefaultMetrics: false });
    });

    it('should return Prometheus-formatted metrics', async () => {
      exporter.recordPacket({ type: 'prepare', status: 'success', latencyMs: 5 });

      const metrics = await exporter.getMetrics();

      // Check for standard Prometheus format elements
      expect(metrics).toContain('# HELP');
      expect(metrics).toContain('# TYPE');
    });

    it('should return correct content type', () => {
      const contentType = exporter.getContentType();

      expect(contentType).toContain('text/plain');
    });
  });

  describe('getMetricsMiddleware', () => {
    beforeEach(async () => {
      exporter = await PrometheusExporter.create(mockLogger, { includeDefaultMetrics: false });
    });

    it('should return Express middleware function', () => {
      const middleware = exporter.getMetricsMiddleware();

      expect(typeof middleware).toBe('function');
    });

    it('should respond with metrics on successful call', async () => {
      exporter.recordPacket({ type: 'prepare', status: 'success', latencyMs: 5 });

      const middleware = exporter.getMetricsMiddleware();
      const mockReq = {} as Request;
      const mockRes = {
        set: jest.fn(),
        send: jest.fn(),
        status: jest.fn().mockReturnThis(),
      } as unknown as Response;

      await middleware(mockReq, mockRes, jest.fn());

      expect(mockRes.set).toHaveBeenCalledWith('Content-Type', expect.any(String));
      expect(mockRes.send).toHaveBeenCalledWith(
        expect.stringContaining('ilp_packets_processed_total')
      );
    });
  });

  describe('reset', () => {
    beforeEach(async () => {
      exporter = await PrometheusExporter.create(mockLogger, { includeDefaultMetrics: false });
    });

    it('should reset all metrics', async () => {
      // Record some metrics
      exporter.recordPacket({ type: 'prepare', status: 'success', latencyMs: 5 });
      exporter.recordSettlement({ method: 'evm', status: 'success', latencyMs: 1000 });

      // Reset
      exporter.reset();

      // SLA metrics should be reset
      const slaMetrics = exporter.getSLAMetrics();
      expect(slaMetrics.packetSuccessRate).toBe(1.0);
      expect(slaMetrics.settlementSuccessRate).toBe(1.0);
      expect(slaMetrics.p99LatencyMs).toBe(0);
    });
  });

  describe('shutdown', () => {
    it('should clear registry on shutdown', async () => {
      exporter = await PrometheusExporter.create(mockLogger, { includeDefaultMetrics: false });

      exporter.shutdown();

      expect(mockLogger.info).toHaveBeenCalledWith('PrometheusExporter shutdown complete');
    });
  });

  describe('claim metrics', () => {
    beforeEach(async () => {
      exporter = await PrometheusExporter.create(mockLogger, { includeDefaultMetrics: false });
    });

    describe('recordClaimSent', () => {
      it('should record claim sent for EVM blockchain', async () => {
        const options: ClaimMetricsOptions = {
          blockchain: 'evm',
          peerId: 'peer-bob',
          success: true,
        };

        exporter.recordClaimSent(options);

        const metrics = await exporter.getMetrics();
        expect(metrics).toContain('claims_sent_total');
        expect(metrics).toContain('peer_id="peer-bob"');
        expect(metrics).toContain('blockchain="evm"');
        expect(metrics).toContain('success="true"');
      });

      it('should record claim sent for another EVM peer', async () => {
        const options: ClaimMetricsOptions = {
          blockchain: 'evm',
          peerId: 'peer-alice',
          success: true,
        };

        exporter.recordClaimSent(options);

        const metrics = await exporter.getMetrics();
        expect(metrics).toContain('blockchain="evm"');
      });

      it('should record failed claim send', async () => {
        const options: ClaimMetricsOptions = {
          blockchain: 'evm',
          peerId: 'peer-bob',
          success: false,
        };

        exporter.recordClaimSent(options);

        const metrics = await exporter.getMetrics();
        expect(metrics).toContain('success="false"');
      });

      it('should warn if success field is missing', () => {
        const options: ClaimMetricsOptions = {
          blockchain: 'evm',
          peerId: 'peer-bob',
        };

        exporter.recordClaimSent(options);

        expect(mockLogger.warn).toHaveBeenCalledWith(
          { blockchain: 'evm', peerId: 'peer-bob' },
          'recordClaimSent called without success field'
        );
      });
    });

    describe('recordClaimReceived', () => {
      it('should record verified claim received', async () => {
        const options: ClaimMetricsOptions = {
          blockchain: 'evm',
          peerId: 'peer-alice',
          verified: true,
        };

        exporter.recordClaimReceived(options);

        const metrics = await exporter.getMetrics();
        expect(metrics).toContain('claims_received_total');
        expect(metrics).toContain('peer_id="peer-alice"');
        expect(metrics).toContain('blockchain="evm"');
        expect(metrics).toContain('verified="true"');
      });

      it('should record unverified claim received', async () => {
        const options: ClaimMetricsOptions = {
          blockchain: 'evm',
          peerId: 'peer-bob',
          verified: false,
        };

        exporter.recordClaimReceived(options);

        const metrics = await exporter.getMetrics();
        expect(metrics).toContain('verified="false"');
      });

      it('should warn if verified field is missing', () => {
        const options: ClaimMetricsOptions = {
          blockchain: 'evm',
          peerId: 'peer-alice',
        };

        exporter.recordClaimReceived(options);

        expect(mockLogger.warn).toHaveBeenCalledWith(
          { blockchain: 'evm', peerId: 'peer-alice' },
          'recordClaimReceived called without verified field'
        );
      });
    });

    describe('recordClaimRedeemed', () => {
      it('should record successful claim redemption with latency', async () => {
        const options: ClaimMetricsOptions = {
          blockchain: 'evm',
          peerId: 'peer-alice',
          success: true,
          latencyMs: 5000,
        };

        exporter.recordClaimRedeemed(options);

        const metrics = await exporter.getMetrics();
        expect(metrics).toContain('claims_redeemed_total');
        expect(metrics).toContain('blockchain="evm"');
        expect(metrics).toContain('success="true"');
        expect(metrics).toContain('claim_redemption_latency_seconds');
      });

      it('should record failed claim redemption', async () => {
        const options: ClaimMetricsOptions = {
          blockchain: 'evm',
          peerId: 'peer-bob',
          success: false,
        };

        exporter.recordClaimRedeemed(options);

        const metrics = await exporter.getMetrics();
        expect(metrics).toContain('success="false"');
      });

      it('should update last redemption timestamp on success', async () => {
        const beforeRecord = Date.now() / 1000;

        const options: ClaimMetricsOptions = {
          blockchain: 'evm',
          peerId: 'peer-alice',
          success: true,
        };

        exporter.recordClaimRedeemed(options);

        const metrics = await exporter.getMetrics();
        expect(metrics).toContain('claim_last_redemption_timestamp_seconds');

        // The timestamp should be recent
        const match = metrics.match(
          /claim_last_redemption_timestamp_seconds\{blockchain="evm"\}\s+(\d+\.?\d*)/
        );
        expect(match).toBeDefined();
        expect(match).not.toBeNull();
        const timestamp = parseFloat(match![1] as string);
        expect(timestamp).toBeGreaterThanOrEqual(beforeRecord);
      });

      it('should not update last redemption timestamp on failure', async () => {
        const options: ClaimMetricsOptions = {
          blockchain: 'evm',
          peerId: 'peer-bob',
          success: false,
        };

        exporter.recordClaimRedeemed(options);

        const metrics = await exporter.getMetrics();
        // Timestamp should not be set for failed redemptions (different peer)
        const match = metrics.match(
          /claim_last_redemption_timestamp_seconds\{blockchain="evm"\}\s+(\d+)/
        );
        // With only failed redemptions, timestamp gauge should not be set
        expect(match).toBeNull();
      });

      it('should record redemption for multiple EVM peers', async () => {
        exporter.recordClaimRedeemed({
          blockchain: 'evm',
          peerId: 'peer-1',
          success: true,
        });

        exporter.recordClaimRedeemed({
          blockchain: 'evm',
          peerId: 'peer-2',
          success: true,
        });

        exporter.recordClaimRedeemed({
          blockchain: 'evm',
          peerId: 'peer-3',
          success: true,
        });

        const metrics = await exporter.getMetrics();
        expect(metrics).toContain('blockchain="evm"');
        expect(metrics).toContain('claims_redeemed_total');
      });

      it('should warn if success field is missing', () => {
        const options: ClaimMetricsOptions = {
          blockchain: 'evm',
          peerId: 'peer-alice',
        };

        exporter.recordClaimRedeemed(options);

        expect(mockLogger.warn).toHaveBeenCalledWith(
          { blockchain: 'evm' },
          'recordClaimRedeemed called without success field'
        );
      });
    });

    describe('recordClaimVerificationFailure', () => {
      it('should record invalid signature verification failure', async () => {
        const options: ClaimMetricsOptions = {
          blockchain: 'evm',
          peerId: 'peer-bob',
          errorType: 'invalid_signature',
        };

        exporter.recordClaimVerificationFailure(options);

        const metrics = await exporter.getMetrics();
        expect(metrics).toContain('claim_verification_failures_total');
        expect(metrics).toContain('peer_id="peer-bob"');
        expect(metrics).toContain('blockchain="evm"');
        expect(metrics).toContain('error_type="invalid_signature"');
      });

      it('should record non-monotonic nonce verification failure', async () => {
        const options: ClaimMetricsOptions = {
          blockchain: 'evm',
          peerId: 'peer-alice',
          errorType: 'non_monotonic_nonce',
        };

        exporter.recordClaimVerificationFailure(options);

        const metrics = await exporter.getMetrics();
        expect(metrics).toContain('error_type="non_monotonic_nonce"');
      });

      it('should record unknown verification failure', async () => {
        const options: ClaimMetricsOptions = {
          blockchain: 'evm',
          peerId: 'peer-charlie',
          errorType: 'unknown',
        };

        exporter.recordClaimVerificationFailure(options);

        const metrics = await exporter.getMetrics();
        expect(metrics).toContain('error_type="unknown"');
      });

      it('should warn if errorType is missing', () => {
        const options: ClaimMetricsOptions = {
          blockchain: 'evm',
          peerId: 'peer-bob',
        };

        exporter.recordClaimVerificationFailure(options);

        expect(mockLogger.warn).toHaveBeenCalledWith(
          { blockchain: 'evm', peerId: 'peer-bob' },
          'recordClaimVerificationFailure called without errorType'
        );
      });
    });
  });
});
