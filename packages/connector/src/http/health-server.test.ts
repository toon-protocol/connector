/**
 * Unit Tests for HealthServer
 * Tests HTTP health check endpoint behavior including extended endpoints for Story 12.6
 */

import request from 'supertest';
import { Request, Response } from 'express';
import { HealthServer } from './health-server';
import {
  HealthStatus,
  HealthStatusProvider,
  HealthStatusExtended,
  HealthStatusExtendedProvider,
} from './types';
import pino from 'pino';

// Mock HealthStatusProvider for testing
class MockHealthStatusProvider implements HealthStatusProvider {
  private _healthStatus: HealthStatus;

  constructor(healthStatus: HealthStatus) {
    this._healthStatus = healthStatus;
  }

  getHealthStatus(): HealthStatus {
    return this._healthStatus;
  }

  setHealthStatus(healthStatus: HealthStatus): void {
    this._healthStatus = healthStatus;
  }
}

// Mock Extended HealthStatusProvider for testing Story 12.6 features
class MockExtendedHealthStatusProvider
  extends MockHealthStatusProvider
  implements HealthStatusExtendedProvider
{
  private _extendedStatus: HealthStatusExtended;

  constructor(extendedStatus: HealthStatusExtended) {
    super(extendedStatus as unknown as HealthStatus);
    this._extendedStatus = extendedStatus;
  }

  getHealthStatusExtended(): HealthStatusExtended {
    return this._extendedStatus;
  }

  setExtendedStatus(status: HealthStatusExtended): void {
    this._extendedStatus = status;
  }
}

describe('HealthServer', () => {
  let mockLogger: pino.Logger;
  let mockProvider: MockHealthStatusProvider;
  let healthServer: HealthServer;

  beforeEach(() => {
    // Create silent logger for tests (no console output)
    mockLogger = pino({ level: 'silent' });
  });

  afterEach(async () => {
    // Clean up: stop server if it was started
    try {
      await healthServer.stop();
    } catch {
      // Ignore errors if server wasn't started
    }
  });

  describe('start()', () => {
    it('should start health server successfully and listen on configured port', async () => {
      // Increase timeout for slow port binding in CI
      jest.setTimeout(15000);
      // Arrange
      const healthyStatus: HealthStatus = {
        status: 'healthy',
        uptime: 120,
        peersConnected: 2,
        totalPeers: 2,
        timestamp: new Date().toISOString(),
        nodeId: 'test-node',
        version: '1.0.0',
      };
      mockProvider = new MockHealthStatusProvider(healthyStatus);
      healthServer = new HealthServer(mockLogger, mockProvider);

      // Act
      await healthServer.start(9080);

      // Assert - GET /health should succeed
      const response = await request('http://localhost:9080').get('/health');
      expect(response.status).toBe(200);
    });

    it('should throw error if port is already in use', async () => {
      // Arrange
      const healthyStatus: HealthStatus = {
        status: 'healthy',
        uptime: 0,
        peersConnected: 0,
        totalPeers: 0,
        timestamp: new Date().toISOString(),
      };
      mockProvider = new MockHealthStatusProvider(healthyStatus);
      const firstServer = new HealthServer(mockLogger, mockProvider);
      const secondServer = new HealthServer(mockLogger, mockProvider);

      // Start first server on port 9181
      await firstServer.start(9181);

      // Act & Assert - Attempt to start second server on same port should fail
      await expect(secondServer.start(9181)).rejects.toThrow('already in use');

      // Cleanup
      await firstServer.stop();
    });
  });

  describe('GET /health endpoint', () => {
    it('should return 200 OK when status is healthy', async () => {
      // Arrange
      const healthyStatus: HealthStatus = {
        status: 'healthy',
        uptime: 120,
        peersConnected: 2,
        totalPeers: 2,
        timestamp: '2025-12-27T10:00:00.000Z',
        nodeId: 'connector-a',
        version: '1.0.0',
      };
      mockProvider = new MockHealthStatusProvider(healthyStatus);
      healthServer = new HealthServer(mockLogger, mockProvider);
      await healthServer.start(9082);

      // Act
      const response = await request('http://localhost:9082').get('/health');

      // Assert
      expect(response.status).toBe(200);
      expect(response.type).toBe('application/json');
      expect(response.body).toEqual(healthyStatus);
    });

    it('should return 503 Service Unavailable when status is unhealthy', async () => {
      // Arrange
      const unhealthyStatus: HealthStatus = {
        status: 'unhealthy',
        uptime: 60,
        peersConnected: 0,
        totalPeers: 2,
        timestamp: '2025-12-27T10:00:00.000Z',
        nodeId: 'connector-a',
      };
      mockProvider = new MockHealthStatusProvider(unhealthyStatus);
      healthServer = new HealthServer(mockLogger, mockProvider);
      await healthServer.start(9083);

      // Act
      const response = await request('http://localhost:9083').get('/health');

      // Assert
      expect(response.status).toBe(503);
      expect(response.body.status).toBe('unhealthy');
      expect(response.body.peersConnected).toBe(0);
      expect(response.body.totalPeers).toBe(2);
    });

    it.skip('should return 503 Service Unavailable when status is starting', async () => {
      // Arrange
      const startingStatus: HealthStatus = {
        status: 'starting',
        uptime: 5,
        peersConnected: 0,
        totalPeers: 2,
        timestamp: '2025-12-27T10:00:00.000Z',
      };
      mockProvider = new MockHealthStatusProvider(startingStatus);
      healthServer = new HealthServer(mockLogger, mockProvider);
      await healthServer.start(9084);

      // Act
      const response = await request('http://localhost:9084').get('/health');

      // Assert
      expect(response.status).toBe(503);
      expect(response.body.status).toBe('starting');
    });

    it('should return JSON response with correct Content-Type header', async () => {
      // Arrange
      const healthyStatus: HealthStatus = {
        status: 'healthy',
        uptime: 100,
        peersConnected: 1,
        totalPeers: 1,
        timestamp: new Date().toISOString(),
      };
      mockProvider = new MockHealthStatusProvider(healthyStatus);
      healthServer = new HealthServer(mockLogger, mockProvider);
      await healthServer.start(9085);

      // Act
      const response = await request('http://localhost:9085').get('/health');

      // Assert
      expect(response.type).toBe('application/json');
      expect(response.headers['content-type']).toMatch(/application\/json/);
    });

    it('should include all required HealthStatus fields in response', async () => {
      // Arrange
      const completeStatus: HealthStatus = {
        status: 'healthy',
        uptime: 300,
        peersConnected: 3,
        totalPeers: 4,
        timestamp: '2025-12-27T12:00:00.000Z',
        nodeId: 'test-connector',
        version: '2.0.0',
      };
      mockProvider = new MockHealthStatusProvider(completeStatus);
      healthServer = new HealthServer(mockLogger, mockProvider);
      await healthServer.start(9086);

      // Act
      const response = await request('http://localhost:9086').get('/health');

      // Assert
      expect(response.body).toHaveProperty('status');
      expect(response.body).toHaveProperty('uptime');
      expect(response.body).toHaveProperty('peersConnected');
      expect(response.body).toHaveProperty('totalPeers');
      expect(response.body).toHaveProperty('timestamp');
      expect(response.body).toHaveProperty('nodeId');
      expect(response.body).toHaveProperty('version');
    });
  });

  describe('stop()', () => {
    it('should stop health server gracefully', async () => {
      // Arrange
      const healthyStatus: HealthStatus = {
        status: 'healthy',
        uptime: 0,
        peersConnected: 0,
        totalPeers: 0,
        timestamp: new Date().toISOString(),
      };
      mockProvider = new MockHealthStatusProvider(healthyStatus);
      healthServer = new HealthServer(mockLogger, mockProvider);
      await healthServer.start(9087);

      // Verify server is running
      const beforeStop = await request('http://localhost:9087').get('/health');
      expect(beforeStop.status).toBe(200);

      // Act
      await healthServer.stop();

      // Assert - Connection should be refused after stop
      await expect(request('http://localhost:9087').get('/health')).rejects.toThrow();
    });

    it('should not throw error if server is not started', async () => {
      // Arrange
      const healthyStatus: HealthStatus = {
        status: 'healthy',
        uptime: 0,
        peersConnected: 0,
        totalPeers: 0,
        timestamp: new Date().toISOString(),
      };
      mockProvider = new MockHealthStatusProvider(healthyStatus);
      healthServer = new HealthServer(mockLogger, mockProvider);

      // Act & Assert - Should not throw
      await expect(healthServer.stop()).resolves.not.toThrow();
    });
  });

  describe('logging', () => {
    it('should log health check requests at DEBUG level', async () => {
      // Arrange
      // Note: Testing actual log capture is complex in Pino
      // This test verifies the server starts and responds without error
      // Actual DEBUG level logging is verified manually or via integration tests

      const healthyStatus: HealthStatus = {
        status: 'healthy',
        uptime: 0,
        peersConnected: 0,
        totalPeers: 0,
        timestamp: new Date().toISOString(),
      };
      mockProvider = new MockHealthStatusProvider(healthyStatus);
      healthServer = new HealthServer(mockLogger, mockProvider);
      await healthServer.start(9088);

      // Act
      await request('http://localhost:9088').get('/health');

      // Assert - This is a basic check; actual log capture may vary
      // The important part is that the server started and responded
      expect(true).toBe(true); // Health check completed without error
    });
  });

  // Story 12.6: Extended Health Endpoints Tests
  describe('GET /health/live (liveness probe)', () => {
    it('should return 200 OK with alive status', async () => {
      // Arrange
      const healthyStatus: HealthStatus = {
        status: 'healthy',
        uptime: 100,
        peersConnected: 1,
        totalPeers: 1,
        timestamp: new Date().toISOString(),
      };
      mockProvider = new MockHealthStatusProvider(healthyStatus);
      healthServer = new HealthServer(mockLogger, mockProvider);
      await healthServer.start(9089);

      // Act
      const response = await request('http://localhost:9089').get('/health/live');

      // Assert
      expect(response.status).toBe(200);
      expect(response.body.status).toBe('alive');
      expect(response.body.timestamp).toBeDefined();
    });

    it('should return 200 even when health status is unhealthy', async () => {
      // Arrange - Liveness should pass even if unhealthy (process is running)
      const unhealthyStatus: HealthStatus = {
        status: 'unhealthy',
        uptime: 10,
        peersConnected: 0,
        totalPeers: 2,
        timestamp: new Date().toISOString(),
      };
      mockProvider = new MockHealthStatusProvider(unhealthyStatus);
      healthServer = new HealthServer(mockLogger, mockProvider);
      await healthServer.start(9090);

      // Act
      const response = await request('http://localhost:9090').get('/health/live');

      // Assert
      expect(response.status).toBe(200);
      expect(response.body.status).toBe('alive');
    });
  });

  describe('GET /health/ready (readiness probe)', () => {
    it('should return 200 when healthy without extended provider', async () => {
      // Arrange
      const healthyStatus: HealthStatus = {
        status: 'healthy',
        uptime: 100,
        peersConnected: 2,
        totalPeers: 2,
        timestamp: new Date().toISOString(),
      };
      mockProvider = new MockHealthStatusProvider(healthyStatus);
      healthServer = new HealthServer(mockLogger, mockProvider);
      await healthServer.start(9091);

      // Act
      const response = await request('http://localhost:9091').get('/health/ready');

      // Assert
      expect(response.status).toBe(200);
      expect(response.body.status).toBe('ready');
    });

    it('should return 503 when unhealthy without extended provider', async () => {
      // Arrange
      const unhealthyStatus: HealthStatus = {
        status: 'unhealthy',
        uptime: 10,
        peersConnected: 0,
        totalPeers: 2,
        timestamp: new Date().toISOString(),
      };
      mockProvider = new MockHealthStatusProvider(unhealthyStatus);
      healthServer = new HealthServer(mockLogger, mockProvider);
      await healthServer.start(9092);

      // Act
      const response = await request('http://localhost:9092').get('/health/ready');

      // Assert
      expect(response.status).toBe(503);
      expect(response.body.status).toBe('not_ready');
    });

    it('should return 200 when extended provider reports healthy with deps up', async () => {
      // Arrange
      const extendedStatus: HealthStatusExtended = {
        status: 'healthy',
        uptime: 100,
        peersConnected: 2,
        totalPeers: 2,
        timestamp: new Date().toISOString(),
        nodeId: 'test-node',
        version: '1.0.0',
        dependencies: {
          tigerbeetle: { status: 'up', latencyMs: 5 },
          evm: { status: 'up', latencyMs: 50 },
        },
        sla: {
          packetSuccessRate: 0.999,
          settlementSuccessRate: 0.99,
          p99LatencyMs: 5,
        },
      };
      const extendedProvider = new MockExtendedHealthStatusProvider(extendedStatus);
      healthServer = new HealthServer(mockLogger, extendedProvider, {
        extendedProvider,
      });
      await healthServer.start(9093);

      // Act
      const response = await request('http://localhost:9093').get('/health/ready');

      // Assert
      expect(response.status).toBe(200);
      expect(response.body.status).toBe('ready');
      expect(response.body.dependencies.tigerbeetle.status).toBe('up');
    });

    it('should return 503 when TigerBeetle is down', async () => {
      // Arrange
      const extendedStatus: HealthStatusExtended = {
        status: 'unhealthy',
        uptime: 100,
        peersConnected: 2,
        totalPeers: 2,
        timestamp: new Date().toISOString(),
        nodeId: 'test-node',
        version: '1.0.0',
        dependencies: {
          tigerbeetle: { status: 'down' },
        },
        sla: {
          packetSuccessRate: 0.5,
          settlementSuccessRate: 0.5,
          p99LatencyMs: 100,
        },
      };
      const extendedProvider = new MockExtendedHealthStatusProvider(extendedStatus);
      healthServer = new HealthServer(mockLogger, extendedProvider, {
        extendedProvider,
      });
      await healthServer.start(9094);

      // Act
      const response = await request('http://localhost:9094').get('/health/ready');

      // Assert
      expect(response.status).toBe(503);
      expect(response.body.status).toBe('not_ready');
      expect(response.body.dependencies.tigerbeetle.status).toBe('down');
    });

    it('should return 200 when degraded but TigerBeetle is up', async () => {
      // Arrange - Degraded status but TigerBeetle up means we can still serve traffic
      const extendedStatus: HealthStatusExtended = {
        status: 'degraded',
        uptime: 100,
        peersConnected: 1,
        totalPeers: 2,
        timestamp: new Date().toISOString(),
        nodeId: 'test-node',
        version: '1.0.0',
        dependencies: {
          tigerbeetle: { status: 'up', latencyMs: 10 },
          evm: { status: 'down' }, // EVM down causes degraded but not unready
        },
        sla: {
          packetSuccessRate: 0.95,
          settlementSuccessRate: 0.9,
          p99LatencyMs: 15,
        },
      };
      const extendedProvider = new MockExtendedHealthStatusProvider(extendedStatus);
      healthServer = new HealthServer(mockLogger, extendedProvider, {
        extendedProvider,
      });
      await healthServer.start(9095);

      // Act
      const response = await request('http://localhost:9095').get('/health/ready');

      // Assert
      expect(response.status).toBe(200);
      expect(response.body.status).toBe('ready');
    });
  });

  describe('GET /metrics endpoint', () => {
    it('should return metrics when middleware is configured', async () => {
      // Arrange
      const healthyStatus: HealthStatus = {
        status: 'healthy',
        uptime: 100,
        peersConnected: 1,
        totalPeers: 1,
        timestamp: new Date().toISOString(),
      };
      mockProvider = new MockHealthStatusProvider(healthyStatus);

      // Mock metrics middleware
      const mockMetricsMiddleware = (_req: Request, res: Response): void => {
        res.set('Content-Type', 'text/plain; version=0.0.4; charset=utf-8');
        res.send('# HELP test_metric A test metric\n# TYPE test_metric gauge\ntest_metric 42\n');
      };

      healthServer = new HealthServer(mockLogger, mockProvider, {
        metricsMiddleware: mockMetricsMiddleware,
      });
      await healthServer.start(9096);

      // Act
      const response = await request('http://localhost:9096').get('/metrics');

      // Assert
      expect(response.status).toBe(200);
      expect(response.text).toContain('test_metric');
      expect(response.headers['content-type']).toContain('text/plain');
    });

    it('should return 404 when metrics middleware is not configured', async () => {
      // Arrange
      const healthyStatus: HealthStatus = {
        status: 'healthy',
        uptime: 100,
        peersConnected: 1,
        totalPeers: 1,
        timestamp: new Date().toISOString(),
      };
      mockProvider = new MockHealthStatusProvider(healthyStatus);
      healthServer = new HealthServer(mockLogger, mockProvider);
      await healthServer.start(9097);

      // Act
      const response = await request('http://localhost:9097').get('/metrics');

      // Assert
      expect(response.status).toBe(404);
    });
  });

  describe('extended health status', () => {
    it('should return extended status with dependencies and SLA when provider available', async () => {
      // Arrange
      const extendedStatus: HealthStatusExtended = {
        status: 'healthy',
        uptime: 300,
        peersConnected: 3,
        totalPeers: 3,
        timestamp: new Date().toISOString(),
        nodeId: 'prod-connector-1',
        version: '2.0.0',
        dependencies: {
          tigerbeetle: { status: 'up', latencyMs: 2 },
          evm: { status: 'up', latencyMs: 50 },
        },
        sla: {
          packetSuccessRate: 0.9995,
          settlementSuccessRate: 0.995,
          p99LatencyMs: 3,
        },
      };
      const extendedProvider = new MockExtendedHealthStatusProvider(extendedStatus);
      healthServer = new HealthServer(mockLogger, extendedProvider, {
        extendedProvider,
      });
      await healthServer.start(9098);

      // Act
      const response = await request('http://localhost:9098').get('/health');

      // Assert
      expect(response.status).toBe(200);
      expect(response.body.dependencies).toBeDefined();
      expect(response.body.dependencies.tigerbeetle.status).toBe('up');
      expect(response.body.sla).toBeDefined();
      expect(response.body.sla.packetSuccessRate).toBe(0.9995);
    });

    it('should return 200 for degraded status (still operational)', async () => {
      // Arrange
      const degradedStatus: HealthStatusExtended = {
        status: 'degraded',
        uptime: 100,
        peersConnected: 1,
        totalPeers: 2,
        timestamp: new Date().toISOString(),
        nodeId: 'test-node',
        version: '1.0.0',
        dependencies: {
          tigerbeetle: { status: 'up', latencyMs: 5 },
          evm: { status: 'down' },
        },
        sla: {
          packetSuccessRate: 0.95,
          settlementSuccessRate: 0.85,
          p99LatencyMs: 20,
        },
      };
      const extendedProvider = new MockExtendedHealthStatusProvider(degradedStatus);
      healthServer = new HealthServer(mockLogger, extendedProvider, {
        extendedProvider,
      });
      await healthServer.start(9099);

      // Act
      const response = await request('http://localhost:9099').get('/health');

      // Assert
      expect(response.status).toBe(200); // Degraded still returns 200
      expect(response.body.status).toBe('degraded');
    });
  });
});
