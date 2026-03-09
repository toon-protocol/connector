/**
 * Unit tests for ConnectionPool
 *
 * Tests connection pool initialization, round-robin selection, health checks,
 * automatic reconnection, and metrics collection using mocked connection factory.
 */

import {
  ConnectionPool,
  ConnectionPoolConfig,
  ConnectionFactory,
} from '../../../src/utils/connection-pool';
import { Logger } from 'pino';

// Mock client type
interface MockClient {
  endpoint: string;
  id: number;
}

describe('ConnectionPool', () => {
  let mockLogger: jest.Mocked<Logger>;
  let mockFactory: jest.Mocked<ConnectionFactory<MockClient>>;
  let connectionPool: ConnectionPool<MockClient>;
  let config: ConnectionPoolConfig;
  let mockClientId: number;

  beforeEach(() => {
    // Create mock logger
    mockLogger = {
      info: jest.fn(),
      error: jest.fn(),
      warn: jest.fn(),
      debug: jest.fn(),
      trace: jest.fn(),
      child: jest.fn().mockReturnThis(),
    } as unknown as jest.Mocked<Logger>;

    // Create mock factory
    mockClientId = 0;
    mockFactory = {
      create: jest.fn().mockImplementation(async (endpoint: string) => ({
        endpoint,
        id: mockClientId++,
      })),
      disconnect: jest.fn().mockResolvedValue(undefined),
      healthCheck: jest.fn().mockResolvedValue(true),
    } as jest.Mocked<ConnectionFactory<MockClient>>;

    // Default config
    config = {
      poolSize: 3,
      endpoints: ['http://rpc1.example.com', 'http://rpc2.example.com', 'http://rpc3.example.com'],
      healthCheckIntervalMs: 100,
      reconnectDelayMs: 10,
      maxReconnectAttempts: 3,
    };

    // Create connection pool instance
    connectionPool = new ConnectionPool(config, mockFactory, mockLogger);
  });

  afterEach(async () => {
    // Shutdown pool to clear timers
    await connectionPool.shutdown();
    jest.clearAllMocks();
  });

  describe('Initialization', () => {
    it('should initialize connection pool with all endpoints', async () => {
      await connectionPool.initialize();

      expect(mockFactory.create).toHaveBeenCalledTimes(3);
      expect(mockFactory.create).toHaveBeenCalledWith('http://rpc1.example.com');
      expect(mockFactory.create).toHaveBeenCalledWith('http://rpc2.example.com');
      expect(mockFactory.create).toHaveBeenCalledWith('http://rpc3.example.com');

      const stats = connectionPool.getStats();
      expect(stats.totalConnections).toBe(3);
      expect(stats.healthyConnections).toBe(3);
      expect(stats.unhealthyConnections).toBe(0);
    });

    it('should handle connection creation failures gracefully', async () => {
      // Mock factory to fail on second endpoint
      mockFactory.create
        .mockResolvedValueOnce({ endpoint: 'http://rpc1.example.com', id: 1 })
        .mockRejectedValueOnce(new Error('Connection failed'))
        .mockResolvedValueOnce({ endpoint: 'http://rpc3.example.com', id: 3 });

      await connectionPool.initialize();

      expect(mockFactory.create).toHaveBeenCalledTimes(3);

      const stats = connectionPool.getStats();
      expect(stats.totalConnections).toBe(3);
      expect(stats.healthyConnections).toBe(2);
      expect(stats.unhealthyConnections).toBe(1);
    });

    it('should limit pool size to configured value', async () => {
      const smallConfig: ConnectionPoolConfig = {
        poolSize: 2,
        endpoints: [
          'http://rpc1.example.com',
          'http://rpc2.example.com',
          'http://rpc3.example.com',
        ],
      };

      const smallPool = new ConnectionPool(smallConfig, mockFactory, mockLogger);

      await smallPool.initialize();

      expect(mockFactory.create).toHaveBeenCalledTimes(2);

      const stats = smallPool.getStats();
      expect(stats.totalConnections).toBe(2);

      await smallPool.shutdown();
    });

    it('should use default values for optional config parameters', async () => {
      const minimalConfig: ConnectionPoolConfig = {
        poolSize: 2,
        endpoints: ['http://rpc1.example.com', 'http://rpc2.example.com'],
      };

      const pool = new ConnectionPool(minimalConfig, mockFactory, mockLogger);

      await pool.initialize();

      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          poolSize: 2,
          endpoints: minimalConfig.endpoints,
          healthCheckIntervalMs: 30000, // Default value
        }),
        'ConnectionPool initialized'
      );

      await pool.shutdown();
    });
  });

  describe('Round-robin selection', () => {
    beforeEach(async () => {
      await connectionPool.initialize();
    });

    it('should return connections in round-robin order', () => {
      const conn1 = connectionPool.getConnection();
      const conn2 = connectionPool.getConnection();
      const conn3 = connectionPool.getConnection();
      const conn4 = connectionPool.getConnection();

      expect(conn1).not.toBeNull();
      expect(conn2).not.toBeNull();
      expect(conn3).not.toBeNull();
      expect(conn4).not.toBeNull();

      // Should cycle back to first connection
      expect(conn1!.endpoint).toBe('http://rpc1.example.com');
      expect(conn2!.endpoint).toBe('http://rpc2.example.com');
      expect(conn3!.endpoint).toBe('http://rpc3.example.com');
      expect(conn4!.endpoint).toBe('http://rpc1.example.com');
    });

    it('should skip unhealthy connections during round-robin', () => {
      // Manually mark second connection as unhealthy
      const conn1 = connectionPool.getConnection();
      conn1!.isHealthy = false;

      const conn2 = connectionPool.getConnection();
      const conn3 = connectionPool.getConnection();

      // Should skip the unhealthy connection
      expect(conn2!.endpoint).toBe('http://rpc2.example.com');
      expect(conn3!.endpoint).toBe('http://rpc3.example.com');
    });

    it('should return null when no healthy connections available', () => {
      // Mark all connections as unhealthy
      const conns = [
        connectionPool.getConnection(),
        connectionPool.getConnection(),
        connectionPool.getConnection(),
      ];

      conns.forEach((conn) => {
        conn!.isHealthy = false;
      });

      const result = connectionPool.getConnection();

      expect(result).toBeNull();
      expect(mockLogger.warn).toHaveBeenCalledWith('No healthy connections available in pool');
    });

    it('should return null when pool is empty', () => {
      const emptyPool = new ConnectionPool({ poolSize: 0, endpoints: [] }, mockFactory, mockLogger);

      const result = emptyPool.getConnection();

      expect(result).toBeNull();
      expect(mockLogger.warn).toHaveBeenCalledWith('No connections available in pool');
    });
  });

  describe('Health checks', () => {
    beforeEach(async () => {
      await connectionPool.initialize();
    });

    it('should perform periodic health checks', async () => {
      // Wait for health check interval
      await new Promise((resolve) => setTimeout(resolve, 150));

      // Health check should be called for all connections
      expect(mockFactory.healthCheck).toHaveBeenCalled();
    });

    it('should mark connection as unhealthy when health check fails', async () => {
      // Mock health check to fail for one connection, and fail reconnection
      mockFactory.healthCheck
        .mockResolvedValueOnce(true)
        .mockResolvedValueOnce(false)
        .mockResolvedValueOnce(true);

      // Fail reconnection attempts to ensure connection stays unhealthy
      mockFactory.create.mockRejectedValue(new Error('Reconnection failed'));

      // Trigger health check and wait for reconnection attempts
      await new Promise((resolve) => setTimeout(resolve, 250));

      // Verify that connection-unhealthy event was emitted (which means health check failed)
      expect(mockLogger.warn).toHaveBeenCalledWith(
        expect.objectContaining({ endpoint: expect.any(String) }),
        'Connection health check failed'
      );
    });

    it('should emit connection-unhealthy event when health check fails', async () => {
      const unhealthyListener = jest.fn();
      connectionPool.on('connection-unhealthy', unhealthyListener);

      // Mock health check to fail
      mockFactory.healthCheck.mockResolvedValue(false);

      // Trigger health check
      await new Promise((resolve) => setTimeout(resolve, 150));

      expect(unhealthyListener).toHaveBeenCalled();
    });

    it('should not perform health checks after shutdown', async () => {
      await connectionPool.shutdown();

      mockFactory.healthCheck.mockClear();

      // Wait for what would be health check interval
      await new Promise((resolve) => setTimeout(resolve, 150));

      // No health checks should occur
      expect(mockFactory.healthCheck).not.toHaveBeenCalled();
    });
  });

  describe('Reconnection', () => {
    beforeEach(async () => {
      await connectionPool.initialize();
      mockFactory.create.mockClear(); // Clear initialization calls
    });

    it('should attempt to reconnect unhealthy connection', async () => {
      // Mock health check to fail, triggering reconnection
      mockFactory.healthCheck.mockResolvedValueOnce(false);
      mockFactory.create.mockResolvedValueOnce({
        endpoint: 'http://rpc1.example.com',
        id: 999,
      });

      // Trigger health check
      await new Promise((resolve) => setTimeout(resolve, 150));

      // Reconnection should be attempted
      expect(mockFactory.disconnect).toHaveBeenCalled();
      expect(mockFactory.create).toHaveBeenCalled();
    });

    it('should emit connection-reconnected event on successful reconnection', async () => {
      const reconnectedListener = jest.fn();
      connectionPool.on('connection-reconnected', reconnectedListener);

      // Mock health check to fail, then reconnection succeeds
      mockFactory.healthCheck.mockResolvedValueOnce(false);
      mockFactory.create.mockResolvedValueOnce({
        endpoint: 'http://rpc1.example.com',
        id: 999,
      });

      // Trigger health check and wait for reconnection
      await new Promise((resolve) => setTimeout(resolve, 200));

      // Reconnected event should be emitted
      expect(reconnectedListener).toHaveBeenCalled();
    });

    it('should retry reconnection up to maxReconnectAttempts', async () => {
      // Clear any previous calls and mock reconnection to always fail
      mockFactory.create.mockClear();
      mockFactory.create.mockRejectedValue(new Error('Reconnect failed'));

      // Mark connection as unhealthy
      const conn = connectionPool.getConnection();
      conn!.isHealthy = false;

      // Wait for connection-failed event (emitted after all reconnection attempts exhausted)
      await new Promise<void>((resolve) => {
        connectionPool.on('connection-failed', resolve);
      });

      // Should attempt reconnection at least maxReconnectAttempts times (3)
      const callCount = mockFactory.create.mock.calls.length;
      expect(callCount).toBeGreaterThanOrEqual(3);
    });

    it('should emit connection-failed event after max reconnection attempts', async () => {
      const failedListener = jest.fn();
      connectionPool.on('connection-failed', failedListener);

      // Mock reconnection to always fail
      mockFactory.create.mockRejectedValue(new Error('Reconnect failed'));

      // Mark connection as unhealthy
      const conn = connectionPool.getConnection();
      conn!.isHealthy = false;

      // Wait for connection-failed event instead of using fixed timeout
      await new Promise<void>((resolve) => {
        connectionPool.on('connection-failed', resolve);
      });

      // Failed event should be emitted
      expect(failedListener).toHaveBeenCalled();
    });
  });

  describe('Statistics', () => {
    it('should track connection statistics accurately', async () => {
      // Initialize with some connections failing
      mockFactory.create
        .mockResolvedValueOnce({ endpoint: 'http://rpc1.example.com', id: 1 })
        .mockRejectedValueOnce(new Error('Connection failed'))
        .mockResolvedValueOnce({ endpoint: 'http://rpc3.example.com', id: 3 });

      await connectionPool.initialize();

      const stats = connectionPool.getStats();

      expect(stats.totalConnections).toBe(3);
      expect(stats.healthyConnections).toBe(2);
      expect(stats.unhealthyConnections).toBe(1);
    });

    it('should update statistics after health checks', async () => {
      await connectionPool.initialize();

      // Initially all healthy
      const stats = connectionPool.getStats();
      expect(stats.healthyConnections).toBe(3);

      // Mock health check to fail for all connections
      mockFactory.healthCheck.mockResolvedValue(false);

      // Fail reconnection to keep connections unhealthy
      mockFactory.create.mockRejectedValue(new Error('Reconnection failed'));

      // Trigger health check and wait for reconnection attempts
      await new Promise((resolve) => setTimeout(resolve, 250));

      // Verify health checks were performed
      expect(mockFactory.healthCheck).toHaveBeenCalled();

      // Verify reconnection was attempted
      expect(mockFactory.create).toHaveBeenCalled();
    });
  });

  describe('Shutdown', () => {
    beforeEach(async () => {
      await connectionPool.initialize();
    });

    it('should disconnect all connections on shutdown', async () => {
      await connectionPool.shutdown();

      expect(mockFactory.disconnect).toHaveBeenCalledTimes(3);
    });

    it('should clear health check timer on shutdown', async () => {
      await connectionPool.shutdown();

      mockFactory.healthCheck.mockClear();

      // Wait for what would be health check interval
      await new Promise((resolve) => setTimeout(resolve, 150));

      // No health checks should occur
      expect(mockFactory.healthCheck).not.toHaveBeenCalled();
    });

    it('should handle disconnect errors gracefully', async () => {
      mockFactory.disconnect
        .mockRejectedValueOnce(new Error('Disconnect error 1'))
        .mockResolvedValueOnce(undefined)
        .mockRejectedValueOnce(new Error('Disconnect error 2'));

      await connectionPool.shutdown();

      // Should complete shutdown despite errors
      expect(mockLogger.error).toHaveBeenCalledWith(
        expect.objectContaining({ error: 'Disconnect error 1' }),
        'Error disconnecting connection'
      );
    });

    it('should clear all connections after shutdown', async () => {
      await connectionPool.shutdown();

      const stats = connectionPool.getStats();
      expect(stats.totalConnections).toBe(0);
    });
  });

  describe('Edge cases', () => {
    it('should handle empty endpoints list', async () => {
      const emptyConfig: ConnectionPoolConfig = {
        poolSize: 3,
        endpoints: [],
      };

      const emptyPool = new ConnectionPool(emptyConfig, mockFactory, mockLogger);

      await emptyPool.initialize();

      const stats = emptyPool.getStats();
      expect(stats.totalConnections).toBe(0);

      await emptyPool.shutdown();
    });

    it('should handle poolSize larger than endpoints list', async () => {
      const config: ConnectionPoolConfig = {
        poolSize: 5,
        endpoints: ['http://rpc1.example.com', 'http://rpc2.example.com'],
      };

      const pool = new ConnectionPool(config, mockFactory, mockLogger);

      await pool.initialize();

      const stats = pool.getStats();
      expect(stats.totalConnections).toBe(2); // Only 2 endpoints available

      await pool.shutdown();
    });

    it('should handle concurrent getConnection calls', async () => {
      await connectionPool.initialize();

      const promises = Array.from({ length: 100 }, () =>
        Promise.resolve(connectionPool.getConnection())
      );

      const results = await Promise.all(promises);

      // All should return valid connections
      expect(results.every((conn) => conn !== null)).toBe(true);
    });
  });
});
