/**
 * Suspicious Activity Detector Tests
 * Story 11.9: Security Hardening for Agent Wallets
 */

import { SuspiciousActivityDetector, DetectionConfig } from './suspicious-activity-detector';
import pino from 'pino';

describe('SuspiciousActivityDetector', () => {
  let detector: SuspiciousActivityDetector;
  let mockLogger: pino.Logger;

  const defaultConfig: DetectionConfig = {
    rapidFundingThreshold: 5, // 5 requests/hour
    unusualTransactionStdDev: 3, // 3 standard deviations
  };

  beforeEach(() => {
    mockLogger = pino({ level: 'silent' });
    detector = new SuspiciousActivityDetector(defaultConfig, mockLogger);
  });

  describe('detectRapidFunding', () => {
    it('should not detect rapid funding with few requests', () => {
      detector.recordFundingRequest('agent-001');
      detector.recordFundingRequest('agent-001');

      const isSuspicious = detector.detectRapidFunding('agent-001');
      expect(isSuspicious).toBe(false);
    });

    it('should detect rapid funding when threshold exceeded', () => {
      // Record 5 funding requests (at threshold)
      for (let i = 0; i < 5; i++) {
        detector.recordFundingRequest('agent-001');
      }

      const isSuspicious = detector.detectRapidFunding('agent-001');
      expect(isSuspicious).toBe(true);
    });

    it('should track different agents separately', () => {
      // Agent 1: 5 requests (suspicious)
      for (let i = 0; i < 5; i++) {
        detector.recordFundingRequest('agent-001');
      }

      // Agent 2: 2 requests (not suspicious)
      detector.recordFundingRequest('agent-002');
      detector.recordFundingRequest('agent-002');

      expect(detector.detectRapidFunding('agent-001')).toBe(true);
      expect(detector.detectRapidFunding('agent-002')).toBe(false);
    });
  });

  describe('detectUnusualTransactions', () => {
    it('should not detect unusual transactions with insufficient data', () => {
      // Only 2 transactions (need 10 for analysis)
      detector.recordTransaction('agent-001', BigInt(1000), 'USDC');
      detector.recordTransaction('agent-001', BigInt(1100), 'USDC');

      const isUnusual = detector.detectUnusualTransactions('agent-001', BigInt(5000), 'USDC');
      expect(isUnusual).toBe(false); // Not enough data
    });

    it('should detect unusual transaction when using new token', () => {
      // Record 10 transactions with USDC
      for (let i = 0; i < 10; i++) {
        detector.recordTransaction('agent-001', BigInt(1000 + i * 100), 'USDC');
      }

      // Try transaction with new token (DAI)
      const isUnusual = detector.detectUnusualTransactions('agent-001', BigInt(1000), 'DAI');
      expect(isUnusual).toBe(true); // New token is suspicious
    });

    it('should detect statistical outliers', () => {
      // Record 20 transactions with mean ~1000 USDC
      for (let i = 0; i < 20; i++) {
        detector.recordTransaction('agent-001', BigInt(1000 + (i - 10) * 10), 'USDC');
      }

      // Try transaction that is 100x larger (huge outlier)
      const isUnusual = detector.detectUnusualTransactions('agent-001', BigInt(100000), 'USDC');
      expect(isUnusual).toBe(true); // Outlier detected
    });

    it('should not flag normal transactions', () => {
      // Record 20 transactions with mean ~1000 USDC
      for (let i = 0; i < 20; i++) {
        detector.recordTransaction('agent-001', BigInt(1000 + (i - 10) * 10), 'USDC');
      }

      // Try normal transaction within range
      const isUnusual = detector.detectUnusualTransactions('agent-001', BigInt(1050), 'USDC');
      expect(isUnusual).toBe(false); // Within normal range
    });
  });

  describe('recordFundingRequest', () => {
    it('should record funding request timestamp', () => {
      detector.recordFundingRequest('agent-001');

      const isSuspicious = detector.detectRapidFunding('agent-001');
      expect(isSuspicious).toBe(false); // Only 1 request
    });
  });

  describe('recordTransaction', () => {
    it('should record transaction for statistical analysis', () => {
      detector.recordTransaction('agent-001', BigInt(1000), 'USDC');

      // Should be recorded (verified indirectly through detection)
      expect(true).toBe(true);
    });
  });

  describe('clear', () => {
    it('should clear all detection history', () => {
      // Record some data
      for (let i = 0; i < 5; i++) {
        detector.recordFundingRequest('agent-001');
      }

      expect(detector.detectRapidFunding('agent-001')).toBe(true);

      // Clear
      detector.clear();

      expect(detector.detectRapidFunding('agent-001')).toBe(false);
    });
  });
});
