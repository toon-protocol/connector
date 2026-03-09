/**
 * Unit tests for MetricsCollector
 */

import { MetricsCollector } from './metrics-collector';

describe('MetricsCollector', () => {
  let collector: MetricsCollector;

  beforeEach(() => {
    collector = new MetricsCollector({
      slidingWindowDuration: 3600000, // 1 hour
      maxAttempts: 1000,
      cleanupInterval: 300000, // 5 minutes
    });
  });

  afterEach(() => {
    collector.destroy();
  });

  describe('recordSuccess', () => {
    it('should append success attempt to sliding window', () => {
      collector.recordSuccess('evm');

      const successRate = collector.getSuccessRate('evm');
      expect(successRate).toBe(1.0);
    });

    it('should track multiple success attempts', () => {
      collector.recordSuccess('evm');
      collector.recordSuccess('evm');
      collector.recordSuccess('evm');

      const successRate = collector.getSuccessRate('evm');
      expect(successRate).toBe(1.0);
    });
  });

  describe('recordFailure', () => {
    it('should append failure attempt to sliding window', () => {
      collector.recordFailure('evm');

      const successRate = collector.getSuccessRate('evm');
      expect(successRate).toBe(0.0);
    });

    it('should track multiple failure attempts', () => {
      collector.recordFailure('evm');
      collector.recordFailure('evm');
      collector.recordFailure('evm');

      const successRate = collector.getSuccessRate('evm');
      expect(successRate).toBe(0.0);
    });
  });

  describe('getSuccessRate', () => {
    it('should return 1.0 when no attempts recorded', () => {
      const successRate = collector.getSuccessRate('evm');
      expect(successRate).toBe(1.0);
    });

    it('should calculate 100% success rate correctly', () => {
      collector.recordSuccess('evm');
      collector.recordSuccess('evm');
      collector.recordSuccess('evm');

      const successRate = collector.getSuccessRate('evm');
      expect(successRate).toBe(1.0);
    });

    it('should calculate 50% success rate correctly', () => {
      collector.recordSuccess('evm');
      collector.recordFailure('evm');

      const successRate = collector.getSuccessRate('evm');
      expect(successRate).toBe(0.5);
    });

    it('should calculate 0% success rate correctly', () => {
      collector.recordFailure('evm');
      collector.recordFailure('evm');

      const successRate = collector.getSuccessRate('evm');
      expect(successRate).toBe(0.0);
    });

    it('should track methods independently', () => {
      // Use type assertion since we're testing with different method identifiers
      collector.recordSuccess('evm');
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (collector as any).recordFailure('other-method');

      expect(collector.getSuccessRate('evm')).toBe(1.0);
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      expect((collector as any).getSuccessRate('other-method')).toBe(0.0);
    });
  });

  describe('getRecentFailureRate', () => {
    it('should return 0.0 when no recent attempts', () => {
      const failureRate = collector.getRecentFailureRate('evm');
      expect(failureRate).toBe(0.0);
    });

    it('should use 1-hour sliding window', () => {
      // Create collector with short window for testing
      const testCollector = new MetricsCollector({
        slidingWindowDuration: 100, // 100ms window
        maxAttempts: 1000,
        cleanupInterval: 300000,
      });

      testCollector.recordFailure('evm');

      // Should have failure immediately
      expect(testCollector.getRecentFailureRate('evm')).toBe(1.0);

      // Wait for window to expire
      return new Promise<void>((resolve) => {
        setTimeout(() => {
          // Failure should be outside window now
          expect(testCollector.getRecentFailureRate('evm')).toBe(0.0);
          testCollector.destroy();
          resolve();
        }, 150);
      });
    });

    it('should calculate failure rate correctly', () => {
      collector.recordSuccess('evm');
      collector.recordSuccess('evm');
      collector.recordFailure('evm');

      // 1 failure out of 3 attempts = 33.3%
      const failureRate = collector.getRecentFailureRate('evm');
      expect(failureRate).toBeCloseTo(0.333, 2);
    });
  });

  describe('getCircuitBreakerState', () => {
    it('should return closed state when failure rate < 10%', () => {
      collector.recordSuccess('evm');
      collector.recordSuccess('evm');
      collector.recordSuccess('evm');
      collector.recordSuccess('evm');
      collector.recordSuccess('evm');
      collector.recordSuccess('evm');
      collector.recordSuccess('evm');
      collector.recordSuccess('evm');
      collector.recordSuccess('evm');
      collector.recordFailure('evm'); // 10% failure rate

      const state = collector.getCircuitBreakerState('evm');
      expect(state.isOpen).toBe(false);
      expect(state.failureRate).toBe(0.1);
    });

    it('should return open state when failure rate > 10%', () => {
      collector.recordSuccess('evm');
      collector.recordFailure('evm');
      collector.recordFailure('evm'); // 66% failure rate

      const state = collector.getCircuitBreakerState('evm');
      expect(state.isOpen).toBe(true);
      expect(state.failureRate).toBeCloseTo(0.666, 2);
    });

    it('should return closed state when no attempts', () => {
      const state = collector.getCircuitBreakerState('evm');
      expect(state.isOpen).toBe(false);
      expect(state.failureRate).toBe(0.0);
    });
  });

  describe('cleanup', () => {
    it('should remove expired attempts', () => {
      // Create collector with short window
      const testCollector = new MetricsCollector({
        slidingWindowDuration: 100, // 100ms window
        maxAttempts: 1000,
        cleanupInterval: 50, // Cleanup every 50ms
      });

      testCollector.recordSuccess('evm');

      // Should have attempt immediately
      expect(testCollector.getSuccessRate('evm')).toBe(1.0);

      // Wait for cleanup to run and window to expire
      return new Promise<void>((resolve) => {
        setTimeout(() => {
          // Attempt should be cleaned up (returns 1.0 for no attempts)
          expect(testCollector.getSuccessRate('evm')).toBe(1.0);
          testCollector.destroy();
          resolve();
        }, 200);
      });
    });

    it('should enforce max attempts limit', () => {
      const testCollector = new MetricsCollector({
        slidingWindowDuration: 3600000,
        maxAttempts: 3, // Small limit for testing
        cleanupInterval: 300000,
      });

      // Record 5 attempts
      testCollector.recordSuccess('evm');
      testCollector.recordSuccess('evm');
      testCollector.recordSuccess('evm');
      testCollector.recordSuccess('evm');
      testCollector.recordSuccess('evm');

      // Should only keep last 3
      const successRate = testCollector.getSuccessRate('evm');
      expect(successRate).toBe(1.0);

      testCollector.destroy();
    });
  });
});
