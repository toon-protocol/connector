/**
 * Unit tests for SettlementCoordinator (EVM-only)
 */
/* eslint-disable @typescript-eslint/no-explicit-any */

import { SettlementCoordinator } from './settlement-coordinator';
import type { MetricsCollector } from './metrics-collector';
import type { PaymentChannelSDK } from './payment-channel-sdk';
import type { Logger } from 'pino';
import type { PeerConfig } from './types';

describe('SettlementCoordinator', () => {
  let coordinator: SettlementCoordinator;
  let mockEVMSDK: jest.Mocked<PaymentChannelSDK>;
  let mockMetricsCollector: jest.Mocked<MetricsCollector>;
  let mockLogger: jest.Mocked<Logger>;
  let peerConfigs: Map<string, PeerConfig>;

  beforeEach(() => {
    // Mock EVM SDK with provider access via type casting
    const mockProvider = {
      getFeeData: jest.fn().mockResolvedValue({ gasPrice: 1000000n }),
    };
    mockEVMSDK = {
      openChannel: jest.fn().mockResolvedValue('evm-channel-123'),
    } as any;
    // Attach provider to mock (accessed via type assertion in implementation)
    (mockEVMSDK as any).provider = mockProvider;

    // Mock MetricsCollector
    mockMetricsCollector = {
      recordSuccess: jest.fn(),
      recordFailure: jest.fn(),
      getSuccessRate: jest.fn().mockReturnValue(0.95),
      getRecentFailureRate: jest.fn().mockReturnValue(0.05),
      getCircuitBreakerState: jest.fn().mockReturnValue({ isOpen: false, failureRate: 0.05 }),
    } as any;

    // Mock Logger
    mockLogger = {
      info: jest.fn(),
      warn: jest.fn(),
      error: jest.fn(),
    } as any;

    // Setup peer configs (EVM-only)
    peerConfigs = new Map();
    peerConfigs.set('peer-alice', {
      peerId: 'peer-alice',
      address: 'g.alice',
      settlementPreference: 'evm',
      settlementTokens: ['USDC', 'M2M'],
      evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb',
    });

    coordinator = new SettlementCoordinator(
      mockEVMSDK,
      mockMetricsCollector,
      { peerConfigs },
      mockLogger
    );
  });

  describe('selectSettlementMethod', () => {
    it('should select EVM for ERC20 token', async () => {
      const result = await coordinator.selectSettlementMethod('peer-alice', 'USDC', 1000n);

      expect(result.method).toBe('evm');
      expect(result.chain).toBe('base-l2');
    });

    it('should filter out methods with circuit breaker open', async () => {
      // Mock EVM circuit breaker open
      mockMetricsCollector.getCircuitBreakerState = jest
        .fn()
        .mockReturnValue({ isOpen: true, failureRate: 0.15 });

      // Try to settle ERC20 token - should throw error because EVM is circuit-broken
      await expect(coordinator.selectSettlementMethod('peer-alice', 'USDC', 1000n)).rejects.toThrow(
        'No available settlement methods'
      );
    });

    it('should throw error when no settlement methods available', async () => {
      // Mock circuit breaker open
      mockMetricsCollector.getCircuitBreakerState = jest
        .fn()
        .mockReturnValue({ isOpen: true, failureRate: 0.15 });

      await expect(coordinator.selectSettlementMethod('peer-alice', 'USDC', 1000n)).rejects.toThrow(
        'No available settlement methods'
      );
    });

    it('should throw error when peer not found', async () => {
      await expect(
        coordinator.selectSettlementMethod('peer-unknown', 'USDC', 1000n)
      ).rejects.toThrow('Peer not found: peer-unknown');
    });
  });

  describe('executeSettlementWithFallback', () => {
    it('should succeed with EVM method', async () => {
      await coordinator.executeSettlementWithFallback('peer-alice', 'USDC', 1000n);

      expect(mockEVMSDK.openChannel).toHaveBeenCalled();
      expect(mockMetricsCollector.recordSuccess).toHaveBeenCalledWith('evm');
    });

    it('should fail when EVM fails with no fallback', async () => {
      // Mock EVM to fail
      mockEVMSDK.openChannel = jest.fn().mockRejectedValue(new Error('EVM RPC timeout'));

      // Should fail with no fallback available
      await expect(
        coordinator.executeSettlementWithFallback('peer-alice', 'USDC', 1000n)
      ).rejects.toThrow('All settlement methods failed');

      expect(mockMetricsCollector.recordFailure).toHaveBeenCalledWith('evm');
    });

    it('should record metrics for failed attempts', async () => {
      // Mock EVM to fail
      mockEVMSDK.openChannel = jest.fn().mockRejectedValue(new Error('EVM failed'));

      await expect(
        coordinator.executeSettlementWithFallback('peer-alice', 'USDC', 1000n)
      ).rejects.toThrow('All settlement methods failed');

      expect(mockMetricsCollector.recordFailure).toHaveBeenCalledWith('evm');
    });
  });

  describe('Circuit Breaker Integration', () => {
    it('should open circuit breaker when failure rate >10%', async () => {
      mockMetricsCollector.getCircuitBreakerState = jest
        .fn()
        .mockReturnValue({ isOpen: true, failureRate: 0.15 });

      await expect(coordinator.selectSettlementMethod('peer-alice', 'USDC', 1000n)).rejects.toThrow(
        'No available settlement methods'
      );

      expect(mockLogger.warn).toHaveBeenCalledWith(
        expect.objectContaining({ method: 'evm', failureRate: 0.15 }),
        'Circuit breaker opened for settlement method'
      );
    });

    it('should keep circuit breaker closed when failure rate <10%', async () => {
      mockMetricsCollector.getCircuitBreakerState = jest
        .fn()
        .mockReturnValue({ isOpen: false, failureRate: 0.05 });

      const result = await coordinator.selectSettlementMethod('peer-alice', 'USDC', 1000n);

      expect(result.available).toBe(true);
    });

    it('should exclude circuit-broken method from options', async () => {
      mockMetricsCollector.getCircuitBreakerState = jest
        .fn()
        .mockReturnValue({ isOpen: true, failureRate: 0.15 });

      // ERC20 token would normally use EVM, but it's circuit-broken
      await expect(coordinator.selectSettlementMethod('peer-alice', 'USDC', 1000n)).rejects.toThrow(
        'No available settlement methods'
      );
    });
  });

  describe('Cost Estimation', () => {
    it('should estimate EVM gas cost for channel claim', async () => {
      (mockEVMSDK as any).provider.getFeeData = jest.fn().mockResolvedValue({ gasPrice: 2000000n });

      const result = await coordinator.selectSettlementMethod('peer-alice', 'USDC', 1000n);

      // 2M gwei * 50k gas units = 100B wei
      expect(result.estimatedCost).toBe(100000000000n);
    });

    it('should cache gas price for 30 seconds', async () => {
      const mockProvider = (mockEVMSDK as any).provider;

      // First call
      await coordinator.selectSettlementMethod('peer-alice', 'USDC', 1000n);
      expect(mockProvider.getFeeData).toHaveBeenCalledTimes(1);

      // Second call within 30 seconds
      await coordinator.selectSettlementMethod('peer-alice', 'USDC', 2000n);
      expect(mockProvider.getFeeData).toHaveBeenCalledTimes(1); // Still only 1 call

      // Wait for cache to expire (31 seconds)
      jest.useFakeTimers();
      jest.advanceTimersByTime(31000);

      await coordinator.selectSettlementMethod('peer-alice', 'USDC', 3000n);
      expect(mockProvider.getFeeData).toHaveBeenCalledTimes(2); // Cache refreshed

      jest.useRealTimers();
    });
  });

  describe('Structured Logging', () => {
    it('should log routing decision with all evaluated options', async () => {
      await coordinator.selectSettlementMethod('peer-alice', 'USDC', 1000n);

      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          peerId: 'peer-alice',
          tokenId: 'USDC',
          selectedMethod: 'evm',
          allOptions: expect.arrayContaining([
            expect.objectContaining({ method: 'evm', available: true }),
          ]),
        }),
        'Settlement routing decision'
      );
    });

    it('should log fallback attempt with error details', async () => {
      mockEVMSDK.openChannel = jest.fn().mockRejectedValue(new Error('EVM network down'));

      await expect(
        coordinator.executeSettlementWithFallback('peer-alice', 'USDC', 1000n)
      ).rejects.toThrow('All settlement methods failed');

      expect(mockLogger.warn).toHaveBeenCalledWith(
        expect.objectContaining({
          peerId: 'peer-alice',
          primaryMethod: 'evm',
          error: 'EVM network down',
        }),
        'Primary settlement failed, trying fallback'
      );
    });

    it('should log circuit breaker state changes', async () => {
      mockMetricsCollector.getCircuitBreakerState = jest
        .fn()
        .mockReturnValue({ isOpen: true, failureRate: 0.15 });

      await expect(coordinator.selectSettlementMethod('peer-alice', 'USDC', 1000n)).rejects.toThrow(
        'No available settlement methods'
      );

      expect(mockLogger.warn).toHaveBeenCalledWith(
        expect.objectContaining({ method: 'evm', failureRate: 0.15 }),
        'Circuit breaker opened for settlement method'
      );
    });
  });

  describe('Token Type Routing', () => {
    it('should select EVM for ERC20 token', async () => {
      const result = await coordinator.selectSettlementMethod('peer-alice', 'USDC', 1000n);

      expect(result.method).toBe('evm');
    });

    it('should handle peer with EVM preference', async () => {
      peerConfigs.set('peer-evm-only', {
        peerId: 'peer-evm-only',
        address: 'g.evm',
        settlementPreference: 'evm',
        settlementTokens: ['USDC', 'M2M'],
        evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb',
      });

      const result = await coordinator.selectSettlementMethod('peer-evm-only', 'USDC', 1000n);

      expect(result.method).toBe('evm');
    });
  });
});
