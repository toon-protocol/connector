/**
 * Unit tests for ConnectorNode
 * @packageDocumentation
 */

import { ConnectorNode } from './connector-node';
import { ConnectorConfig } from '../config/types';
import { RoutingTable } from '../routing/routing-table';
import { BTPClientManager } from '../btp/btp-client-manager';
import { BTPServer } from '../btp/btp-server';
import { PacketHandler } from './packet-handler';
import { Logger } from '../utils/logger';
import {
  RoutingTableEntry,
  PacketType,
  ILPErrorCode,
  ILPFulfillPacket,
  ILPRejectPacket,
} from '@crosstown/shared';
import { ConfigLoader, ConnectorNotStartedError } from '../config/config-loader';
import { HealthServer } from '../http/health-server';

// Mock all dependencies
jest.mock('../routing/routing-table');
jest.mock('../btp/btp-client-manager');
jest.mock('../btp/btp-server');
jest.mock('./packet-handler');
jest.mock('../config/config-loader', () => {
  const actual = jest.requireActual('../config/config-loader');
  return {
    ...actual,
    ConfigLoader: {
      loadConfig: jest.fn(),
      validateConfig: jest.fn(),
    },
  };
});
jest.mock('../http/health-server');
jest.mock('../http/admin-api', () => ({
  validateSettlementConfig: jest.fn().mockReturnValue(null),
}));

/**
 * Mock logger for testing
 */
const createMockLogger = (): jest.Mocked<Logger> =>
  ({
    info: jest.fn(),
    warn: jest.fn(),
    error: jest.fn(),
    debug: jest.fn(),
    fatal: jest.fn(),
    trace: jest.fn(),
    silent: jest.fn(),
    level: 'info',
    child: jest.fn().mockReturnThis(),
  }) as unknown as jest.Mocked<Logger>;

/**
 * Create test connector configuration
 */
const createTestConfig = (overrides?: Partial<ConnectorConfig>): ConnectorConfig => {
  const testPeer = {
    id: 'peerA',
    url: 'ws://connector-a:3000',
    authToken: 'secret-a',
  };

  const testRoute: RoutingTableEntry = {
    prefix: 'g.peerA',
    nextHop: 'peerA',
  };

  return {
    nodeId: 'connector-test',
    btpServerPort: 3000,
    environment: 'development',
    peers: [testPeer],
    routes: [testRoute],
    ...overrides,
  };
};

describe('ConnectorNode', () => {
  let connectorNode: ConnectorNode;
  let mockLogger: jest.Mocked<Logger>;
  let mockRoutingTable: jest.Mocked<RoutingTable>;
  let mockBTPClientManager: jest.Mocked<BTPClientManager>;
  let mockBTPServer: jest.Mocked<BTPServer>;
  let mockPacketHandler: jest.Mocked<PacketHandler>;
  let mockHealthServer: jest.Mocked<HealthServer>;
  let config: ConnectorConfig;
  const testConfigPath = '/test/config.yaml';

  beforeEach(() => {
    jest.clearAllMocks();
    mockLogger = createMockLogger();
    config = createTestConfig();

    // Mock ConfigLoader to return our test config
    (ConfigLoader.loadConfig as jest.Mock) = jest.fn().mockReturnValue(config);

    // Create mocked instances
    mockRoutingTable = {
      lookup: jest.fn(),
      getAllRoutes: jest.fn().mockReturnValue(config.routes),
      addRoute: jest.fn(),
      removeRoute: jest.fn(),
    } as unknown as jest.Mocked<RoutingTable>;

    mockBTPClientManager = {
      addPeer: jest.fn().mockResolvedValue(undefined),
      removePeer: jest.fn().mockResolvedValue(undefined),
      sendToPeer: jest.fn(),
      getPeerStatus: jest.fn().mockReturnValue(new Map([['peerA', true]])),
      getPeerIds: jest.fn().mockReturnValue(['peerA']),
      isConnected: jest.fn().mockReturnValue(true),
      getConnectedPeerCount: jest.fn().mockReturnValue(1),
      getTotalPeerCount: jest.fn().mockReturnValue(1),
      getConnectionHealth: jest.fn().mockReturnValue(100),
      setPacketHandler: jest.fn(),
    } as unknown as jest.Mocked<BTPClientManager>;

    mockBTPServer = {
      start: jest.fn().mockResolvedValue(undefined),
      stop: jest.fn().mockResolvedValue(undefined),
    } as unknown as jest.Mocked<BTPServer>;

    mockPacketHandler = {
      processPrepare: jest.fn(),
      setBTPServer: jest.fn(),
      setLocalDeliveryHandler: jest.fn(),
      setLocalDelivery: jest.fn(),
      handlePreparePacket: jest.fn(),
    } as unknown as jest.Mocked<PacketHandler>;

    mockHealthServer = {
      start: jest.fn().mockResolvedValue(undefined),
      stop: jest.fn().mockResolvedValue(undefined),
    } as unknown as jest.Mocked<HealthServer>;

    // Configure mocks to return our mocked instances
    (RoutingTable as jest.MockedClass<typeof RoutingTable>).mockImplementation(
      () => mockRoutingTable
    );
    (BTPClientManager as jest.MockedClass<typeof BTPClientManager>).mockImplementation(
      () => mockBTPClientManager
    );
    (BTPServer as jest.MockedClass<typeof BTPServer>).mockImplementation(() => mockBTPServer);
    (PacketHandler as jest.MockedClass<typeof PacketHandler>).mockImplementation(
      () => mockPacketHandler
    );
    (HealthServer as jest.MockedClass<typeof HealthServer>).mockImplementation(
      () => mockHealthServer
    );
  });

  describe('Constructor', () => {
    it('should create ConnectorNode with all components', () => {
      // Arrange & Act
      connectorNode = new ConnectorNode(testConfigPath, mockLogger);

      // Assert
      expect(connectorNode).toBeDefined();
      expect(connectorNode).toBeInstanceOf(ConnectorNode);
      expect(ConfigLoader.loadConfig).toHaveBeenCalledWith(testConfigPath);
      expect(mockLogger.child).toHaveBeenCalledWith({
        component: 'ConnectorNode',
        nodeId: 'connector-test',
      });
    });

    it('should initialize RoutingTable with config routes', () => {
      // Arrange & Act
      connectorNode = new ConnectorNode(testConfigPath, mockLogger);

      // Assert
      expect(RoutingTable).toHaveBeenCalledWith(
        expect.arrayContaining([expect.objectContaining({ prefix: 'g.peerA', nextHop: 'peerA' })]),
        expect.anything() // child logger
      );
    });

    it('should initialize BTPClientManager with logger', () => {
      // Arrange & Act
      connectorNode = new ConnectorNode(testConfigPath, mockLogger);

      // Assert
      expect(BTPClientManager).toHaveBeenCalledWith(config.nodeId, expect.anything());
    });

    it('should initialize PacketHandler with dependencies', () => {
      // Arrange & Act
      connectorNode = new ConnectorNode(testConfigPath, mockLogger);

      // Assert
      expect(PacketHandler).toHaveBeenCalledWith(
        mockRoutingTable,
        mockBTPClientManager,
        config.nodeId,
        expect.anything() // child logger
      );
    });

    it('should initialize BTPServer with PacketHandler', () => {
      // Arrange & Act
      connectorNode = new ConnectorNode(testConfigPath, mockLogger);

      // Assert
      expect(BTPServer).toHaveBeenCalledWith(
        expect.anything(), // child logger
        mockPacketHandler
      );
    });

    it('should initialize HealthServer with logger and provider', () => {
      // Arrange & Act
      connectorNode = new ConnectorNode(testConfigPath, mockLogger);

      // Assert
      expect(HealthServer).toHaveBeenCalledWith(
        expect.anything(), // child logger
        connectorNode // ConnectorNode implements HealthStatusProvider
      );
    });

    it('should log config_loaded and connector_initialized events', () => {
      // Arrange & Act
      connectorNode = new ConnectorNode(testConfigPath, mockLogger);

      // Assert
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'config_loaded',
          filePath: testConfigPath,
          nodeId: 'connector-test',
        }),
        expect.any(String)
      );
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'connector_initialized',
          nodeId: 'connector-test',
          peersCount: 1,
          routesCount: 1,
        }),
        expect.any(String)
      );
    });
  });

  describe('start()', () => {
    beforeEach(() => {
      connectorNode = new ConnectorNode(testConfigPath, mockLogger);
      jest.clearAllMocks(); // Clear constructor logs
    });

    it('should start BTP server first, then health server, then clients', async () => {
      // Arrange
      const startOrder: string[] = [];
      mockBTPServer.start.mockImplementation(async () => {
        startOrder.push('btp-server');
      });
      mockHealthServer.start.mockImplementation(async () => {
        startOrder.push('health-server');
      });
      mockBTPClientManager.addPeer.mockImplementation(async () => {
        startOrder.push('client');
      });

      // Act
      await connectorNode.start();

      // Assert
      expect(startOrder[0]).toBe('btp-server');
      expect(startOrder[1]).toBe('health-server');
      expect(startOrder[2]).toBe('client');
      expect(mockBTPServer.start).toHaveBeenCalledWith(3000);
      expect(mockHealthServer.start).toHaveBeenCalledWith(8080);
    });

    it('should connect all BTP clients in parallel', async () => {
      // Arrange
      const configWithMultiplePeers = createTestConfig({
        peers: [
          {
            id: 'peerA',
            url: 'ws://connector-a:3000',
            authToken: 'secret-a',
          },
          {
            id: 'peerB',
            url: 'ws://connector-b:3001',
            authToken: 'secret-b',
          },
        ],
      });
      (ConfigLoader.loadConfig as jest.Mock).mockReturnValue(configWithMultiplePeers);
      connectorNode = new ConnectorNode(testConfigPath, mockLogger);
      jest.clearAllMocks();

      // Act
      await connectorNode.start();

      // Assert
      expect(mockBTPClientManager.addPeer).toHaveBeenCalledTimes(2);
      expect(mockBTPClientManager.addPeer).toHaveBeenCalledWith(
        expect.objectContaining({ id: 'peerA' })
      );
      expect(mockBTPClientManager.addPeer).toHaveBeenCalledWith(
        expect.objectContaining({ id: 'peerB' })
      );
    });

    it('should log connector_starting, btp_server_started, health_server_started, and connector_ready events', async () => {
      // Arrange & Act
      await connectorNode.start();

      // Assert
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'connector_starting',
          nodeId: 'connector-test',
        }),
        expect.any(String)
      );
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'btp_server_started',
          port: 3000,
        }),
        expect.any(String)
      );
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'health_server_started',
          port: 8080,
        }),
        expect.any(String)
      );
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'connector_ready',
          nodeId: 'connector-test',
          connectedPeers: 1,
          totalPeers: 1,
        }),
        expect.any(String)
      );
    });

    it('should set status to healthy on successful start with all peers connected', async () => {
      // Arrange & Act
      await connectorNode.start();
      const healthStatus = connectorNode.getHealthStatus();

      // Assert
      expect(healthStatus.status).toBe('healthy');
    });

    it('should log error and set status to unhealthy on start failure', async () => {
      // Arrange
      const testError = new Error('BTP server start failed');
      mockBTPServer.start.mockRejectedValue(testError);

      // Act & Assert
      await expect(connectorNode.start()).rejects.toThrow('BTP server start failed');
      expect(mockLogger.error).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'connector_start_failed',
          nodeId: 'connector-test',
          error: 'BTP server start failed',
        }),
        expect.any(String)
      );

      const healthStatus = connectorNode.getHealthStatus();
      expect(healthStatus.status).toBe('unhealthy');
    });
  });

  describe('stop()', () => {
    beforeEach(async () => {
      connectorNode = new ConnectorNode(testConfigPath, mockLogger);
      jest.clearAllMocks();
      await connectorNode.start();
      jest.clearAllMocks();
      // Re-apply default mock return values after clearing
      mockBTPClientManager.getPeerIds.mockReturnValue(['peerA']);
      mockBTPClientManager.getPeerStatus.mockReturnValue(new Map([['peerA', true]]));
      mockBTPClientManager.removePeer.mockResolvedValue(undefined);
      mockHealthServer.stop.mockResolvedValue(undefined);
      mockBTPServer.stop.mockResolvedValue(undefined);
    });

    it('should disconnect all BTP clients', async () => {
      // Arrange
      mockBTPClientManager.getPeerIds.mockReturnValue(['peerA', 'peerB']);

      // Act
      await connectorNode.stop();

      // Assert
      expect(mockBTPClientManager.removePeer).toHaveBeenCalledTimes(2);
      expect(mockBTPClientManager.removePeer).toHaveBeenCalledWith('peerA');
      expect(mockBTPClientManager.removePeer).toHaveBeenCalledWith('peerB');
    });

    it('should stop health server and BTP server after disconnecting clients', async () => {
      // Arrange
      const stopOrder: string[] = [];
      mockBTPClientManager.removePeer.mockImplementation(async () => {
        stopOrder.push('client');
      });
      mockHealthServer.stop.mockImplementation(async () => {
        stopOrder.push('health-server');
      });
      mockBTPServer.stop.mockImplementation(async () => {
        stopOrder.push('btp-server');
      });

      // Act
      await connectorNode.stop();

      // Assert
      expect(stopOrder[0]).toBe('client');
      expect(stopOrder).toContain('health-server');
      expect(stopOrder).toContain('btp-server');
      expect(mockHealthServer.stop).toHaveBeenCalledTimes(1);
      expect(mockBTPServer.stop).toHaveBeenCalledTimes(1);
    });

    it('should log connector_stopping and connector_stopped events', async () => {
      // Arrange & Act
      await connectorNode.stop();

      // Assert
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'connector_stopping',
          nodeId: 'connector-test',
        }),
        expect.any(String)
      );
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'connector_stopped',
          nodeId: 'connector-test',
        }),
        expect.any(String)
      );
    });

    it('should reset status to starting after successful stop', async () => {
      // Arrange — connector already started in beforeEach

      // Act
      await connectorNode.stop();
      const healthStatus = connectorNode.getHealthStatus();

      // Assert
      expect(healthStatus.status).toBe('starting');
      expect(healthStatus.peersConnected).toBe(1); // BTPClientManager mock still returns 1
    });

    it('should log error on stop failure', async () => {
      // Arrange
      const testError = new Error('Failed to disconnect peer');
      mockBTPClientManager.removePeer.mockRejectedValue(testError);

      // Act & Assert
      await expect(connectorNode.stop()).rejects.toThrow('Failed to disconnect peer');
      expect(mockLogger.error).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'connector_stop_failed',
          nodeId: 'connector-test',
          error: 'Failed to disconnect peer',
        }),
        expect.any(String)
      );
    });
  });

  describe('getHealthStatus() - Task 8: Health Integration Tests', () => {
    beforeEach(() => {
      connectorNode = new ConnectorNode(testConfigPath, mockLogger);
      jest.clearAllMocks();
    });

    it('Test 1: ConnectorNode implements HealthStatusProvider interface', () => {
      // Arrange & Act
      const healthStatus = connectorNode.getHealthStatus();

      // Assert - should return HealthStatus object with all required fields
      expect(healthStatus).toBeDefined();
      expect(healthStatus).toHaveProperty('status');
      expect(healthStatus).toHaveProperty('uptime');
      expect(healthStatus).toHaveProperty('peersConnected');
      expect(healthStatus).toHaveProperty('totalPeers');
      expect(healthStatus).toHaveProperty('timestamp');
      expect(healthStatus).toHaveProperty('nodeId');
      expect(healthStatus).toHaveProperty('version');

      // Verify types
      expect(typeof healthStatus.status).toBe('string');
      expect(typeof healthStatus.uptime).toBe('number');
      expect(typeof healthStatus.peersConnected).toBe('number');
      expect(typeof healthStatus.totalPeers).toBe('number');
      expect(typeof healthStatus.timestamp).toBe('string');
      expect(typeof healthStatus.nodeId).toBe('string');
      expect(typeof healthStatus.version).toBe('string');
    });

    it('Test 2: Health status is "starting" during initialization', () => {
      // Arrange & Act - before start() is called
      const healthStatus = connectorNode.getHealthStatus();

      // Assert
      expect(healthStatus.status).toBe('starting');
      expect(healthStatus.nodeId).toBe('connector-test');
    });

    it('Test 3: Health status is "healthy" when all peers connected (100%)', async () => {
      // Arrange
      mockBTPClientManager.getPeerStatus.mockReturnValue(new Map([['peerA', true]]));

      // Act
      await connectorNode.start();
      const healthStatus = connectorNode.getHealthStatus();

      // Assert
      expect(healthStatus.status).toBe('healthy');
      expect(healthStatus.peersConnected).toBe(1);
      expect(healthStatus.totalPeers).toBe(1);
    });

    it('Test 4: Health status is "unhealthy" when <50% peers connected', async () => {
      // Arrange - Configure 4 peers, only 1 connected (25%)
      const configWithManyPeers = createTestConfig({
        peers: [
          { id: 'peer1', url: 'ws://p1:3000', authToken: 'token1' },
          { id: 'peer2', url: 'ws://p2:3000', authToken: 'token2' },
          { id: 'peer3', url: 'ws://p3:3000', authToken: 'token3' },
          { id: 'peer4', url: 'ws://p4:3000', authToken: 'token4' },
        ],
      });
      (ConfigLoader.loadConfig as jest.Mock).mockReturnValue(configWithManyPeers);
      connectorNode = new ConnectorNode(testConfigPath, mockLogger);

      // Mock only 1 out of 4 peers connected
      mockBTPClientManager.getPeerStatus.mockReturnValue(
        new Map([
          ['peer1', true],
          ['peer2', false],
          ['peer3', false],
          ['peer4', false],
        ])
      );

      // Act
      jest.clearAllMocks();
      await connectorNode.start();
      const healthStatus = connectorNode.getHealthStatus();

      // Assert
      expect(healthStatus.status).toBe('unhealthy');
      expect(healthStatus.peersConnected).toBe(1);
      expect(healthStatus.totalPeers).toBe(4);
    });

    it('Test 5: Uptime increases over time', async () => {
      // Arrange
      await connectorNode.start();

      // Act - Get initial uptime
      const healthStatus1 = connectorNode.getHealthStatus();
      const uptime1 = healthStatus1.uptime;

      // Wait 1100ms (just over 1 second to ensure uptime counter increases)
      await new Promise((resolve) => setTimeout(resolve, 1100));

      // Get uptime again
      const healthStatus2 = connectorNode.getHealthStatus();
      const uptime2 = healthStatus2.uptime;

      // Assert - uptime is in seconds, so should increase by at least 1
      expect(uptime2).toBeGreaterThan(uptime1);
      expect(uptime2 - uptime1).toBeGreaterThanOrEqual(1);
    });

    it('Test 6: Health server starts and stops with ConnectorNode', async () => {
      // Arrange & Act - Start
      await connectorNode.start();

      // Assert - Health server should have been started
      expect(mockHealthServer.start).toHaveBeenCalledTimes(1);
      expect(mockHealthServer.start).toHaveBeenCalledWith(8080);

      // Act - Stop
      await connectorNode.stop();

      // Assert - Health server should have been stopped
      expect(mockHealthServer.stop).toHaveBeenCalledTimes(1);
    });

    it('Test 7: Health status changes logged at INFO level', async () => {
      // Arrange - Start with peers disconnected (<50%)
      const configWith2Peers = createTestConfig({
        peers: [
          { id: 'peer1', url: 'ws://p1:3000', authToken: 'token1' },
          { id: 'peer2', url: 'ws://p2:3000', authToken: 'token2' },
        ],
      });
      (ConfigLoader.loadConfig as jest.Mock).mockReturnValue(configWith2Peers);
      connectorNode = new ConnectorNode(testConfigPath, mockLogger);

      // Mock only 1 out of 2 peers connected (50% - should be healthy at boundary)
      mockBTPClientManager.getPeerStatus.mockReturnValue(
        new Map([
          ['peer1', true],
          ['peer2', false],
        ])
      );
      jest.clearAllMocks();

      // Act - Start connector (should trigger health status change from 'starting' to 'unhealthy')
      await connectorNode.start();

      // Assert - Should log health_status_changed event at INFO level
      const healthStatusChangedLogs = (mockLogger.info as jest.Mock).mock.calls.filter(
        (call) => call[0]?.event === 'health_status_changed'
      );

      expect(healthStatusChangedLogs.length).toBeGreaterThan(0);
    });

    it('Test 8: Health status "healthy" when no peers configured (standalone mode)', async () => {
      // Arrange - Configure connector with no peers
      const configNoPeers = createTestConfig({
        peers: [],
      });
      (ConfigLoader.loadConfig as jest.Mock).mockReturnValue(configNoPeers);
      connectorNode = new ConnectorNode(testConfigPath, mockLogger);

      mockBTPClientManager.getPeerStatus.mockReturnValue(new Map());
      jest.clearAllMocks();

      // Act
      await connectorNode.start();
      const healthStatus = connectorNode.getHealthStatus();

      // Assert - Standalone mode should be healthy
      expect(healthStatus.status).toBe('healthy');
      expect(healthStatus.peersConnected).toBe(0);
      expect(healthStatus.totalPeers).toBe(0);
    });

    it('Test 9: Health status includes nodeId and version from package.json', () => {
      // Arrange & Act
      const healthStatus = connectorNode.getHealthStatus();

      // Assert
      expect(healthStatus.nodeId).toBe('connector-test');
      expect(healthStatus.version).toBeDefined();
      expect(typeof healthStatus.version).toBe('string');
    });

    it('Test 10: Timestamp is valid ISO 8601 format', () => {
      // Arrange & Act
      const healthStatus = connectorNode.getHealthStatus();

      // Assert
      expect(healthStatus.timestamp).toBeDefined();
      expect(() => new Date(healthStatus.timestamp)).not.toThrow();

      const timestamp = new Date(healthStatus.timestamp);
      expect(timestamp.toISOString()).toBe(healthStatus.timestamp);
    });
  });

  describe('Object-based Construction', () => {
    it('should initialize successfully with a valid ConnectorConfig object', () => {
      // Arrange
      (ConfigLoader.validateConfig as jest.Mock) = jest.fn().mockReturnValue(config);

      // Act
      connectorNode = new ConnectorNode(config, mockLogger);

      // Assert
      expect(connectorNode).toBeDefined();
      expect(connectorNode).toBeInstanceOf(ConnectorNode);
      expect(ConfigLoader.validateConfig).toHaveBeenCalledWith(config);
      expect(ConfigLoader.loadConfig).not.toHaveBeenCalled();
    });

    it('should call ConfigLoader.loadConfig when constructed with a string path', () => {
      // Arrange & Act
      connectorNode = new ConnectorNode(testConfigPath, mockLogger);

      // Assert
      expect(ConfigLoader.loadConfig).toHaveBeenCalledWith(testConfigPath);
    });

    it('should throw ConfigurationError for invalid config object missing nodeId', () => {
      // Arrange - use the same ConfigurationError that ConnectorNode imports (mocked module)
      const { ConfigurationError } = jest.requireActual('../config/config-loader');
      (ConfigLoader.validateConfig as jest.Mock) = jest.fn().mockImplementation(() => {
        throw new ConfigurationError('Missing required field: nodeId');
      });
      const invalidConfig = { btpServerPort: 3000, peers: [], routes: [] };

      // Act & Assert
      expect(
        () => new ConnectorNode(invalidConfig as unknown as ConnectorConfig, mockLogger)
      ).toThrow('Missing required field: nodeId');
    });

    it('should throw ConfigurationError for invalid config object missing peers', () => {
      // Arrange
      const { ConfigurationError: RealConfigError } = jest.requireActual('../config/config-loader');
      (ConfigLoader.validateConfig as jest.Mock) = jest.fn().mockImplementation(() => {
        throw new RealConfigError('Missing required field: peers');
      });
      const invalidConfig = { nodeId: 'test', btpServerPort: 3000, routes: [] };

      // Act & Assert
      expect(
        () => new ConnectorNode(invalidConfig as unknown as ConnectorConfig, mockLogger)
      ).toThrow('Missing required field: peers');
    });

    it('should throw ConfigurationError for invalid port range in config object', () => {
      // Arrange
      const { ConfigurationError: RealConfigError } = jest.requireActual('../config/config-loader');
      (ConfigLoader.validateConfig as jest.Mock) = jest.fn().mockImplementation(() => {
        throw new RealConfigError('BTP server port must be between 1-65535, got: 99999');
      });
      const invalidConfig = {
        nodeId: 'test',
        btpServerPort: 99999,
        peers: [],
        routes: [],
      };

      // Act & Assert
      expect(
        () => new ConnectorNode(invalidConfig as unknown as ConnectorConfig, mockLogger)
      ).toThrow('BTP server port must be between 1-65535');
    });

    it('should log source as "object" when constructed with config object', () => {
      // Arrange
      (ConfigLoader.validateConfig as jest.Mock) = jest.fn().mockReturnValue(config);

      // Act
      connectorNode = new ConnectorNode(config, mockLogger);

      // Assert
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'config_loaded',
          source: 'object',
          nodeId: 'connector-test',
        }),
        expect.any(String)
      );
    });
  });

  describe('setLocalDeliveryHandler()', () => {
    beforeEach(() => {
      connectorNode = new ConnectorNode(testConfigPath, mockLogger);
      jest.clearAllMocks();
    });

    it('should set the handler and propagate to PacketHandler', () => {
      // Arrange
      const handler = jest
        .fn()
        .mockResolvedValue({ fulfill: { fulfillment: 'dGVzdA==', data: '' } });

      // Act
      connectorNode.setLocalDeliveryHandler(handler);

      // Assert
      expect(mockPacketHandler.setLocalDeliveryHandler).toHaveBeenCalledWith(handler);
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'local_delivery_handler_set',
          hasHandler: true,
        }),
        'Local delivery function handler registered'
      );
    });

    it('should be callable before start()', () => {
      // Arrange
      const handler = jest.fn().mockResolvedValue({ fulfill: { fulfillment: 'dGVzdA==' } });

      // Act - call before start()
      connectorNode.setLocalDeliveryHandler(handler);

      // Assert - no errors, handler propagated
      expect(mockPacketHandler.setLocalDeliveryHandler).toHaveBeenCalledWith(handler);
    });

    it('should be callable after construction (handler propagated to PacketHandler)', async () => {
      // Arrange
      const handler = jest.fn().mockResolvedValue({ fulfill: { fulfillment: 'dGVzdA==' } });
      await connectorNode.start();
      jest.clearAllMocks();

      // Act
      connectorNode.setLocalDeliveryHandler(handler);

      // Assert
      expect(mockPacketHandler.setLocalDeliveryHandler).toHaveBeenCalledWith(handler);
    });

    it('should clear the handler when called with null (reverts to HTTP fallback)', () => {
      // Arrange
      const handler = jest.fn().mockResolvedValue({ fulfill: { fulfillment: 'dGVzdA==' } });
      connectorNode.setLocalDeliveryHandler(handler);
      jest.clearAllMocks();

      // Act
      connectorNode.setLocalDeliveryHandler(null);

      // Assert
      expect(mockPacketHandler.setLocalDeliveryHandler).toHaveBeenCalledWith(null);
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'local_delivery_handler_set',
          hasHandler: false,
        }),
        'Local delivery function handler cleared'
      );
    });
  });

  describe('setPacketHandler()', () => {
    beforeEach(() => {
      connectorNode = new ConnectorNode(testConfigPath, mockLogger);
      jest.clearAllMocks();
    });

    it('should wrap handler and propagate to PacketHandler', () => {
      // Arrange
      const handler = jest.fn().mockResolvedValue({ accept: true });

      // Act
      connectorNode.setPacketHandler(handler);

      // Assert — should have called setLocalDeliveryHandler with a function (the adapter)
      expect(mockPacketHandler.setLocalDeliveryHandler).toHaveBeenCalledWith(expect.any(Function));
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'packet_handler_set',
          hasHandler: true,
        }),
        'Packet handler registered'
      );
    });

    it('should clear the handler when called with null', () => {
      // Arrange — set a handler first
      connectorNode.setPacketHandler(jest.fn().mockResolvedValue({ accept: true }));
      jest.clearAllMocks();

      // Act
      connectorNode.setPacketHandler(null);

      // Assert
      expect(mockPacketHandler.setLocalDeliveryHandler).toHaveBeenCalledWith(null);
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'packet_handler_set',
          hasHandler: false,
        }),
        'Packet handler cleared'
      );
    });

    it('should be callable before start()', () => {
      // Arrange
      const handler = jest.fn().mockResolvedValue({ accept: true });

      // Act — call before start()
      connectorNode.setPacketHandler(handler);

      // Assert — no errors, adapter propagated
      expect(mockPacketHandler.setLocalDeliveryHandler).toHaveBeenCalledWith(expect.any(Function));
    });

    it('should overwrite previous setLocalDeliveryHandler (last writer wins)', () => {
      // Arrange — set a raw local delivery handler first
      const rawHandler = jest.fn().mockResolvedValue({ fulfill: { fulfillment: 'dGVzdA==' } });
      connectorNode.setLocalDeliveryHandler(rawHandler);
      jest.clearAllMocks();

      // Act — now set a payment handler, should overwrite
      connectorNode.setPacketHandler(jest.fn().mockResolvedValue({ accept: true }));

      // Assert — setLocalDeliveryHandler called with new adapter (not the raw handler)
      expect(mockPacketHandler.setLocalDeliveryHandler).toHaveBeenCalledTimes(1);
      const calledWith = mockPacketHandler.setLocalDeliveryHandler.mock.calls[0]![0];
      expect(calledWith).not.toBe(rawHandler);
      expect(typeof calledWith).toBe('function');
    });
  });

  describe('sendPacket()', () => {
    const validParams = {
      destination: 'g.peerA.alice',
      amount: 1000n,
      executionCondition: Buffer.alloc(32, 0xab),
      expiresAt: new Date(Date.now() + 30000),
    };

    const createMockFulfill = (): ILPFulfillPacket => ({
      type: PacketType.FULFILL as const,
      fulfillment: Buffer.alloc(32, 0xcd),
      data: Buffer.alloc(0),
    });

    const createMockReject = (code = ILPErrorCode.F02_UNREACHABLE): ILPRejectPacket => ({
      type: PacketType.REJECT as const,
      code,
      triggeredBy: 'connector-test',
      message: 'No route found',
      data: Buffer.alloc(0),
    });

    beforeEach(() => {
      connectorNode = new ConnectorNode(testConfigPath, mockLogger);
      jest.clearAllMocks();
    });

    it('should route packet through PacketHandler.handlePreparePacket()', async () => {
      // Arrange
      await connectorNode.start();
      jest.clearAllMocks();
      mockPacketHandler.handlePreparePacket.mockResolvedValue(createMockFulfill());

      // Act
      await connectorNode.sendPacket(validParams);

      // Assert
      expect(mockPacketHandler.handlePreparePacket).toHaveBeenCalledWith(
        expect.objectContaining({
          type: PacketType.PREPARE,
          destination: validParams.destination,
          amount: validParams.amount,
          executionCondition: validParams.executionCondition,
          expiresAt: validParams.expiresAt,
        }),
        'connector-test' // nodeId as fromPeerId
      );
    });

    it('should return Fulfill on successful routing', async () => {
      // Arrange
      await connectorNode.start();
      jest.clearAllMocks();
      const mockFulfill = createMockFulfill();
      mockPacketHandler.handlePreparePacket.mockResolvedValue(mockFulfill);

      // Act
      const result = await connectorNode.sendPacket(validParams);

      // Assert
      expect(result).toBe(mockFulfill);
    });

    it('should return Reject on routing failure (no route)', async () => {
      // Arrange
      await connectorNode.start();
      jest.clearAllMocks();
      const mockReject = createMockReject();
      mockPacketHandler.handlePreparePacket.mockResolvedValue(mockReject);

      // Act
      const result = await connectorNode.sendPacket(validParams);

      // Assert
      expect(result).toBe(mockReject);
      expect(result.type).toBe(PacketType.REJECT);
    });

    it('should throw ConnectorNotStartedError before start()', async () => {
      // Arrange - do NOT call start()

      // Act & Assert
      await expect(connectorNode.sendPacket(validParams)).rejects.toThrow(ConnectorNotStartedError);
      await expect(connectorNode.sendPacket(validParams)).rejects.toThrow(
        'Connector is not started. Call start() before sendPacket().'
      );
    });

    it('should throw ConnectorNotStartedError after stop()', async () => {
      // Arrange
      await connectorNode.start();
      await connectorNode.stop();

      // Act & Assert
      await expect(connectorNode.sendPacket(validParams)).rejects.toThrow(ConnectorNotStartedError);
    });

    it('should construct ILPPreparePacket with correct fields', async () => {
      // Arrange
      await connectorNode.start();
      jest.clearAllMocks();
      mockPacketHandler.handlePreparePacket.mockResolvedValue(createMockFulfill());

      // Act - send without optional data
      await connectorNode.sendPacket(validParams);

      // Assert
      const calls = mockPacketHandler.handlePreparePacket.mock.calls;
      expect(calls.length).toBe(1);
      const packet = calls[0]![0];
      expect(packet.type).toBe(PacketType.PREPARE);
      expect(packet.destination).toBe(validParams.destination);
      expect(packet.amount).toBe(validParams.amount);
      expect(packet.executionCondition).toBe(validParams.executionCondition);
      expect(packet.expiresAt).toBe(validParams.expiresAt);
      expect(packet.data).toEqual(Buffer.alloc(0)); // default when not provided
    });

    it('should forward custom data payload', async () => {
      // Arrange
      await connectorNode.start();
      jest.clearAllMocks();
      mockPacketHandler.handlePreparePacket.mockResolvedValue(createMockFulfill());
      const customData = Buffer.from('test-payload');

      // Act
      await connectorNode.sendPacket({ ...validParams, data: customData });

      // Assert
      const calls = mockPacketHandler.handlePreparePacket.mock.calls;
      expect(calls.length).toBe(1);
      const packet = calls[0]![0];
      expect(packet.data).toEqual(Buffer.from('test-payload'));
    });

    it('should return T00 Reject on unexpected handlePreparePacket error', async () => {
      // Arrange
      await connectorNode.start();
      jest.clearAllMocks();
      mockPacketHandler.handlePreparePacket.mockRejectedValue(new Error('something broke'));

      // Act
      const result = await connectorNode.sendPacket(validParams);

      // Assert
      expect(result.type).toBe(PacketType.REJECT);
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      expect((result as any).code).toBe(ILPErrorCode.T00_INTERNAL_ERROR);
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      expect((result as any).triggeredBy).toBe('connector-test');
      expect(mockLogger.error).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'send_packet_error',
          destination: validParams.destination,
        }),
        expect.any(String)
      );
    });

    it('should log send_packet event', async () => {
      // Arrange
      await connectorNode.start();
      jest.clearAllMocks();
      mockPacketHandler.handlePreparePacket.mockResolvedValue(createMockFulfill());

      // Act
      await connectorNode.sendPacket(validParams);

      // Assert
      expect(mockLogger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          event: 'send_packet',
          destination: validParams.destination,
          amount: validParams.amount.toString(),
        }),
        'Sending packet via public API'
      );
    });
  });

  describe('getRoutingTable()', () => {
    beforeEach(() => {
      connectorNode = new ConnectorNode(testConfigPath, mockLogger);
    });

    it('should return routing table entries', () => {
      // Arrange
      const expectedRoutes: RoutingTableEntry[] = [
        { prefix: 'g.peerA', nextHop: 'peerA' },
        { prefix: 'g.peerB', nextHop: 'peerB' },
      ];
      mockRoutingTable.getAllRoutes.mockReturnValue(expectedRoutes);

      // Act
      const routes = connectorNode.getRoutingTable();

      // Assert
      expect(routes).toEqual(expectedRoutes);
      expect(mockRoutingTable.getAllRoutes).toHaveBeenCalledTimes(1);
    });
  });

  describe('admin operations', () => {
    beforeEach(async () => {
      connectorNode = new ConnectorNode(testConfigPath, mockLogger);
      jest.clearAllMocks();
      // Re-apply mocks after clearAllMocks
      mockRoutingTable.getAllRoutes.mockReturnValue([
        { prefix: 'g.peerA', nextHop: 'peerA', priority: 0 },
      ]);
      mockBTPClientManager.getPeerIds.mockReturnValue(['peerA']);
      mockBTPClientManager.getPeerStatus.mockReturnValue(new Map([['peerA', true]]));
      mockBTPClientManager.isConnected.mockReturnValue(true);
      // Start connector to enable lifecycle checks
      await connectorNode.start();
      jest.clearAllMocks();
      // Re-apply mocks after second clearAllMocks
      mockRoutingTable.getAllRoutes.mockReturnValue([
        { prefix: 'g.peerA', nextHop: 'peerA', priority: 0 },
      ]);
      mockBTPClientManager.getPeerIds.mockReturnValue(['peerA']);
      mockBTPClientManager.getPeerStatus.mockReturnValue(new Map([['peerA', true]]));
      mockBTPClientManager.isConnected.mockReturnValue(true);
    });

    // ── registerPeer() ──

    it('registerPeer() adds a new peer via BTPClientManager', async () => {
      // Arrange
      mockBTPClientManager.getPeerIds.mockReturnValue(['peerA']); // peerB not in list yet
      mockBTPClientManager.isConnected.mockReturnValue(false);
      mockRoutingTable.getAllRoutes.mockReturnValue([]);

      // Act
      const result = await connectorNode.registerPeer({
        id: 'peerB',
        url: 'ws://peer-b:3000',
        authToken: 'token-b',
      });

      // Assert
      expect(mockBTPClientManager.addPeer).toHaveBeenCalledWith(
        expect.objectContaining({
          id: 'peerB',
          url: 'ws://peer-b:3000',
          authToken: 'token-b',
          connected: false,
        })
      );
      expect(result.id).toBe('peerB');
    });

    it('registerPeer() adds routes for new peer', async () => {
      // Arrange
      mockBTPClientManager.getPeerIds.mockReturnValue([]); // new peer
      mockBTPClientManager.isConnected.mockReturnValue(false);
      mockRoutingTable.getAllRoutes.mockReturnValue([]);

      // Act
      await connectorNode.registerPeer({
        id: 'peerB',
        url: 'ws://peer-b:3000',
        authToken: 'token-b',
        routes: [{ prefix: 'g.peerB', priority: 10 }, { prefix: 'g.peerB.sub' }],
      });

      // Assert
      expect(mockRoutingTable.addRoute).toHaveBeenCalledTimes(2);
      expect(mockRoutingTable.addRoute).toHaveBeenCalledWith('g.peerB', 'peerB', 10);
      expect(mockRoutingTable.addRoute).toHaveBeenCalledWith('g.peerB.sub', 'peerB', 0);
    });

    it('registerPeer() throws ConnectorNotStartedError before start()', async () => {
      // Arrange - create fresh connector, do NOT start
      const freshConnector = new ConnectorNode(testConfigPath, mockLogger);

      // Act & Assert
      await expect(
        freshConnector.registerPeer({
          id: 'peerB',
          url: 'ws://peer-b:3000',
          authToken: 'token-b',
        })
      ).rejects.toThrow(ConnectorNotStartedError);
    });

    it('registerPeer() validates URL format', async () => {
      // Act & Assert
      await expect(
        connectorNode.registerPeer({
          id: 'peerB',
          url: 'http://invalid',
          authToken: 'token',
        })
      ).rejects.toThrow('URL must start with ws:// or wss://');
    });

    it('registerPeer() handles re-registration (idempotent)', async () => {
      // Arrange - peerA already exists
      mockBTPClientManager.getPeerIds.mockReturnValue(['peerA']);
      mockRoutingTable.getAllRoutes.mockReturnValue([
        { prefix: 'g.peerA', nextHop: 'peerA', priority: 0 },
      ]);

      // Act
      const result = await connectorNode.registerPeer({
        id: 'peerA',
        url: 'ws://connector-a:3000',
        authToken: 'secret-a',
        routes: [{ prefix: 'g.peerA.new' }],
      });

      // Assert - addPeer NOT called (re-registration)
      expect(mockBTPClientManager.addPeer).not.toHaveBeenCalled();
      // But routes ARE added
      expect(mockRoutingTable.addRoute).toHaveBeenCalledWith('g.peerA.new', 'peerA', 0);
      expect(result.id).toBe('peerA');
    });

    it('registerPeer() validates ILP address prefix in routes', async () => {
      // Arrange
      mockBTPClientManager.getPeerIds.mockReturnValue([]);

      // Act & Assert
      await expect(
        connectorNode.registerPeer({
          id: 'peerB',
          url: 'ws://peer-b:3000',
          authToken: 'token',
          routes: [{ prefix: 'INVALID PREFIX!!!' }],
        })
      ).rejects.toThrow('Invalid ILP address prefix: INVALID PREFIX!!!');
    });

    // ── removePeer() ──

    it('removePeer() disconnects and removes a peer, returns RemovePeerResult', async () => {
      // Arrange
      mockBTPClientManager.getPeerIds.mockReturnValue(['peerA']);
      mockRoutingTable.getAllRoutes.mockReturnValue([
        { prefix: 'g.peerA', nextHop: 'peerA', priority: 0 },
      ]);

      // Act
      const result = await connectorNode.removePeer('peerA');

      // Assert
      expect(mockBTPClientManager.removePeer).toHaveBeenCalledWith('peerA');
      expect(result.peerId).toBe('peerA');
      expect(result.removedRoutes).toContain('g.peerA');
    });

    it('removePeer() removes associated routes when removeRoutes=true and returns prefixes', async () => {
      // Arrange
      mockBTPClientManager.getPeerIds.mockReturnValue(['peerA']);
      mockRoutingTable.getAllRoutes.mockReturnValue([
        { prefix: 'g.peerA', nextHop: 'peerA', priority: 0 },
        { prefix: 'g.peerA.sub', nextHop: 'peerA', priority: 0 },
        { prefix: 'g.other', nextHop: 'otherPeer', priority: 0 },
      ]);

      // Act
      const result = await connectorNode.removePeer('peerA', true);

      // Assert
      expect(mockRoutingTable.removeRoute).toHaveBeenCalledTimes(2);
      expect(mockRoutingTable.removeRoute).toHaveBeenCalledWith('g.peerA');
      expect(mockRoutingTable.removeRoute).toHaveBeenCalledWith('g.peerA.sub');
      expect(result.removedRoutes).toEqual(['g.peerA', 'g.peerA.sub']);
    });

    it('removePeer() returns empty removedRoutes when removeRoutes=false', async () => {
      // Arrange
      mockBTPClientManager.getPeerIds.mockReturnValue(['peerA']);

      // Act
      const result = await connectorNode.removePeer('peerA', false);

      // Assert
      expect(mockRoutingTable.removeRoute).not.toHaveBeenCalled();
      expect(result.removedRoutes).toEqual([]);
    });

    it('removePeer() throws Error for non-existent peer', async () => {
      // Arrange
      mockBTPClientManager.getPeerIds.mockReturnValue([]);

      // Act & Assert
      await expect(connectorNode.removePeer('unknown')).rejects.toThrow('Peer not found: unknown');
    });

    it('removePeer() throws ConnectorNotStartedError before start()', async () => {
      // Arrange
      const freshConnector = new ConnectorNode(testConfigPath, mockLogger);

      // Act & Assert
      await expect(freshConnector.removePeer('peerA')).rejects.toThrow(ConnectorNotStartedError);
    });

    // ── listPeers() ──

    it('listPeers() returns all peers with connection status', () => {
      // Arrange
      mockBTPClientManager.getPeerIds.mockReturnValue(['peerA', 'peerB']);
      mockBTPClientManager.getPeerStatus.mockReturnValue(
        new Map([
          ['peerA', true],
          ['peerB', false],
        ])
      );
      mockRoutingTable.getAllRoutes.mockReturnValue([
        { prefix: 'g.peerA', nextHop: 'peerA', priority: 0 },
        { prefix: 'g.peerB', nextHop: 'peerB', priority: 0 },
        { prefix: 'g.peerB.sub', nextHop: 'peerB', priority: 5 },
      ]);

      // Act
      const peers = connectorNode.listPeers();

      // Assert
      expect(peers).toHaveLength(2);

      const peerA = peers.find((p) => p.id === 'peerA');
      expect(peerA).toBeDefined();
      expect(peerA!.connected).toBe(true);
      expect(peerA!.ilpAddresses).toEqual(['g.peerA']);
      expect(peerA!.routeCount).toBe(1);

      const peerB = peers.find((p) => p.id === 'peerB');
      expect(peerB).toBeDefined();
      expect(peerB!.connected).toBe(false);
      expect(peerB!.ilpAddresses).toEqual(['g.peerB', 'g.peerB.sub']);
      expect(peerB!.routeCount).toBe(2);
    });

    // ── getBalance() ──

    it('getBalance() returns balance from AccountManager', async () => {
      // Arrange - access private _accountManager and set mock
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (connectorNode as any)._accountManager = {
        getAccountBalance: jest.fn().mockResolvedValue({
          debitBalance: 100n,
          creditBalance: 200n,
          netBalance: -100n,
        }),
      };

      // Act
      const result = await connectorNode.getBalance('peerA', 'M2M');

      // Assert
      expect(result.peerId).toBe('peerA');
      expect(result.balances).toHaveLength(1);
      expect(result.balances[0]).toEqual({
        tokenId: 'M2M',
        debitBalance: '100',
        creditBalance: '200',
        netBalance: '-100',
      });
    });

    it('getBalance() throws when account management not enabled', async () => {
      // Arrange - ensure _accountManager is null (default)
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (connectorNode as any)._accountManager = null;

      // Act & Assert
      await expect(connectorNode.getBalance('peerA')).rejects.toThrow(
        'Account management not enabled'
      );
    });

    // ── listRoutes() ──

    it('listRoutes() returns all routes from routing table', () => {
      // Arrange
      mockRoutingTable.getAllRoutes.mockReturnValue([
        { prefix: 'g.peerA', nextHop: 'peerA', priority: 0 },
        { prefix: 'g.peerB', nextHop: 'peerB', priority: 5 },
      ]);

      // Act
      const routes = connectorNode.listRoutes();

      // Assert
      expect(routes).toEqual([
        { prefix: 'g.peerA', nextHop: 'peerA', priority: 0 },
        { prefix: 'g.peerB', nextHop: 'peerB', priority: 5 },
      ]);
    });

    // ── addRoute() ──

    it('addRoute() adds route to routing table', () => {
      // Act
      connectorNode.addRoute({ prefix: 'g.test', nextHop: 'peerA', priority: 10 });

      // Assert
      expect(mockRoutingTable.addRoute).toHaveBeenCalledWith('g.test', 'peerA', 10);
    });

    it('addRoute() validates ILP address format', () => {
      // Act & Assert
      expect(() =>
        connectorNode.addRoute({ prefix: 'INVALID!!!', nextHop: 'peerA', priority: 0 })
      ).toThrow('Invalid ILP address prefix: INVALID!!!');
    });

    it('addRoute() validates nextHop is not empty', () => {
      // Act & Assert
      expect(() => connectorNode.addRoute({ prefix: 'g.test', nextHop: '', priority: 0 })).toThrow(
        'Missing or invalid nextHop'
      );
    });

    // ── removeRoute() ──

    it('removeRoute() removes route from routing table', () => {
      // Arrange
      mockRoutingTable.getAllRoutes.mockReturnValue([
        { prefix: 'g.peerA', nextHop: 'peerA', priority: 0 },
      ]);

      // Act
      connectorNode.removeRoute('g.peerA');

      // Assert
      expect(mockRoutingTable.removeRoute).toHaveBeenCalledWith('g.peerA');
    });

    it('removeRoute() throws Error for non-existent route', () => {
      // Arrange
      mockRoutingTable.getAllRoutes.mockReturnValue([]);

      // Act & Assert
      expect(() => connectorNode.removeRoute('g.nonexistent')).toThrow(
        'Route not found: g.nonexistent'
      );
    });
  });

  describe('openChannel()', () => {
    beforeEach(async () => {
      connectorNode = new ConnectorNode(testConfigPath, mockLogger);
      jest.clearAllMocks();
      mockBTPClientManager.getPeerIds.mockReturnValue(['peerA']);
      mockBTPClientManager.getPeerStatus.mockReturnValue(new Map([['peerA', true]]));
      await connectorNode.start();
      jest.clearAllMocks();
      mockBTPClientManager.getPeerIds.mockReturnValue(['peerA']);
    });

    it('throws ConnectorNotStartedError if not started', async () => {
      const freshConnector = new ConnectorNode(testConfigPath, mockLogger);
      await expect(
        freshConnector.openChannel({
          peerId: 'peerA',
          chain: 'evm:base:8453',
          peerAddress: '0x' + 'ab'.repeat(20),
        })
      ).rejects.toThrow(ConnectorNotStartedError);
    });

    it('throws if settlement infrastructure not enabled (_channelManager is null)', async () => {
      // _channelManager is null by default (no settlement env vars set)
      await expect(
        connectorNode.openChannel({
          peerId: 'peerA',
          chain: 'evm:base:8453',
          peerAddress: '0x' + 'ab'.repeat(20),
        })
      ).rejects.toThrow('Settlement infrastructure not enabled');
    });

    it('throws if peer not registered', async () => {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (connectorNode as any)._channelManager = {
        getChannelForPeer: jest.fn(),
        ensureChannelExists: jest.fn(),
        getChannelById: jest.fn(),
      };
      mockBTPClientManager.getPeerIds.mockReturnValue([]); // no peers

      await expect(
        connectorNode.openChannel({
          peerId: 'unknown-peer',
          chain: 'evm:base:8453',
          peerAddress: '0x' + 'ab'.repeat(20),
        })
      ).rejects.toThrow("Peer 'unknown-peer' must be registered before opening channels");
    });

    it('throws if active channel already exists for peer+token', async () => {
      const mockChannelManager = {
        getChannelForPeer: jest.fn().mockReturnValue({ status: 'open' }),
        ensureChannelExists: jest.fn(),
        getChannelById: jest.fn(),
      };
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (connectorNode as any)._channelManager = mockChannelManager;

      await expect(
        connectorNode.openChannel({
          peerId: 'peerA',
          chain: 'evm:base:8453',
          peerAddress: '0x' + 'ab'.repeat(20),
        })
      ).rejects.toThrow('Channel already exists for peer peerA with token AGENT');
    });

    it('calls channelManager.ensureChannelExists() with correct params and returns result', async () => {
      const mockChannelManager = {
        getChannelForPeer: jest.fn().mockReturnValue(null),
        ensureChannelExists: jest.fn().mockResolvedValue('0xchannel123'),
        getChannelById: jest.fn().mockReturnValue({
          channelId: '0xchannel123',
          status: 'open',
          chain: 'evm:base:8453',
        }),
      };
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (connectorNode as any)._channelManager = mockChannelManager;

      const result = await connectorNode.openChannel({
        peerId: 'peerA',
        chain: 'evm:base:8453',
        peerAddress: '0x' + 'ab'.repeat(20),
        initialDeposit: '5000',
        settlementTimeout: 3600,
        token: 'M2M',
      });

      expect(mockChannelManager.ensureChannelExists).toHaveBeenCalledWith('peerA', 'M2M', {
        initialDeposit: 5000n,
        settlementTimeout: 3600,
        chain: 'evm:base:8453',
        peerAddress: '0x' + 'ab'.repeat(20),
      });
      expect(result).toEqual({ channelId: '0xchannel123', status: 'open' });
    });

    it('uses default tokenId AGENT when token not provided', async () => {
      const mockChannelManager = {
        getChannelForPeer: jest.fn().mockReturnValue(null),
        ensureChannelExists: jest.fn().mockResolvedValue('0xchannel456'),
        getChannelById: jest.fn().mockReturnValue({
          channelId: '0xchannel456',
          status: 'open',
          chain: 'evm:base:8453',
        }),
      };
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (connectorNode as any)._channelManager = mockChannelManager;

      await connectorNode.openChannel({
        peerId: 'peerA',
        chain: 'evm:base:8453',
        peerAddress: '0x' + 'ab'.repeat(20),
      });

      expect(mockChannelManager.ensureChannelExists).toHaveBeenCalledWith(
        'peerA',
        'AGENT',
        expect.objectContaining({ initialDeposit: 0n })
      );
    });

    it('uses default initialDeposit 0 when not provided', async () => {
      const mockChannelManager = {
        getChannelForPeer: jest.fn().mockReturnValue(null),
        ensureChannelExists: jest.fn().mockResolvedValue('0xchannel789'),
        getChannelById: jest.fn().mockReturnValue({
          channelId: '0xchannel789',
          status: 'opening',
          chain: 'evm:base:8453',
        }),
      };
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (connectorNode as any)._channelManager = mockChannelManager;

      await connectorNode.openChannel({
        peerId: 'peerA',
        chain: 'evm:base:8453',
        peerAddress: '0x' + 'ab'.repeat(20),
      });

      expect(mockChannelManager.ensureChannelExists).toHaveBeenCalledWith(
        'peerA',
        'AGENT',
        expect.objectContaining({ initialDeposit: 0n })
      );
    });
  });

  describe('getChannelState()', () => {
    beforeEach(async () => {
      connectorNode = new ConnectorNode(testConfigPath, mockLogger);
      jest.clearAllMocks();
      mockBTPClientManager.getPeerIds.mockReturnValue(['peerA']);
      mockBTPClientManager.getPeerStatus.mockReturnValue(new Map([['peerA', true]]));
      await connectorNode.start();
      jest.clearAllMocks();
    });

    it('throws ConnectorNotStartedError if not started', async () => {
      const freshConnector = new ConnectorNode(testConfigPath, mockLogger);
      await expect(freshConnector.getChannelState('0xchannel123')).rejects.toThrow(
        ConnectorNotStartedError
      );
    });

    it('throws if settlement infrastructure not enabled', async () => {
      // _channelManager is null by default
      await expect(connectorNode.getChannelState('0xchannel123')).rejects.toThrow(
        'Settlement infrastructure not enabled'
      );
    });

    it('throws if channel not found', async () => {
      const mockChannelManager = {
        getChannelById: jest.fn().mockReturnValue(null),
      };
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (connectorNode as any)._channelManager = mockChannelManager;

      await expect(connectorNode.getChannelState('0xnonexistent')).rejects.toThrow(
        'Channel not found: 0xnonexistent'
      );
    });

    it('returns { channelId, status, chain } from channel metadata', async () => {
      const mockChannelManager = {
        getChannelById: jest.fn().mockReturnValue({
          channelId: '0xchannel123',
          status: 'active', // will be normalized to 'open'
          chain: 'evm:base:8453',
          peerId: 'peerA',
          tokenId: 'AGENT',
        }),
      };
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (connectorNode as any)._channelManager = mockChannelManager;

      const result = await connectorNode.getChannelState('0xchannel123');

      expect(result).toEqual({
        channelId: '0xchannel123',
        status: 'open',
        chain: 'evm:base:8453',
      });
    });
  });

  describe('Lifecycle — reentrant and idempotent', () => {
    beforeEach(() => {
      connectorNode = new ConnectorNode(testConfigPath, mockLogger);
      jest.clearAllMocks();
    });

    it('stop() is idempotent — calling stop() twice does not throw', async () => {
      // Arrange
      await connectorNode.start();
      jest.clearAllMocks();

      // Act — stop twice in sequence
      await connectorNode.stop();
      await connectorNode.stop();

      // Assert — no error thrown, second call is a no-op
    });

    it('stop() on never-started connector does not throw', async () => {
      // Act & Assert — stop without start, should return without error
      await expect(connectorNode.stop()).resolves.toBeUndefined();
    });

    it('start() → stop() → start() lifecycle works (reentrant)', async () => {
      // Arrange & Act — full lifecycle cycle
      await connectorNode.start();
      await connectorNode.stop();
      await connectorNode.start();

      // Assert — healthy after second start
      const healthStatus = connectorNode.getHealthStatus();
      expect(healthStatus.status).toBe('healthy');
    });

    it('start() throws on BTP server failure and sets health to unhealthy', async () => {
      // Arrange
      const testError = new Error('BTP server start failed');
      mockBTPServer.start.mockRejectedValue(testError);

      // Act & Assert
      await expect(connectorNode.start()).rejects.toThrow('BTP server start failed');
      const healthStatus = connectorNode.getHealthStatus();
      expect(healthStatus.status).toBe('unhealthy');
    });

    it('stop() shuts down SettlementMonitor when active', async () => {
      // Arrange
      await connectorNode.start();
      jest.clearAllMocks();

      const mockSettlementMonitor = {
        stop: jest.fn().mockResolvedValue(undefined),
      };
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (connectorNode as any)._settlementMonitor = mockSettlementMonitor;

      // Act
      await connectorNode.stop();

      // Assert
      expect(mockSettlementMonitor.stop).toHaveBeenCalledTimes(1);
    });

    it('stop() shuts down SettlementExecutor when active', async () => {
      // Arrange
      await connectorNode.start();
      jest.clearAllMocks();

      const mockSettlementExecutor = {
        stop: jest.fn(),
      };
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (connectorNode as any)._settlementExecutor = mockSettlementExecutor;

      // Act
      await connectorNode.stop();

      // Assert
      expect(mockSettlementExecutor.stop).toHaveBeenCalledTimes(1);
    });

    it('stop() closes TigerBeetle client when connected', async () => {
      // Arrange
      await connectorNode.start();
      jest.clearAllMocks();

      const mockTigerBeetleClient = {
        close: jest.fn().mockResolvedValue(undefined),
      };
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (connectorNode as any)._tigerBeetleClient = mockTigerBeetleClient;

      // Act
      await connectorNode.stop();

      // Assert
      expect(mockTigerBeetleClient.close).toHaveBeenCalledTimes(1);
    });

    it('stop() shuts down SettlementExecutor before ChannelManager', async () => {
      // Arrange
      await connectorNode.start();
      jest.clearAllMocks();

      const callOrder: string[] = [];
      const mockSettlementExecutor = {
        stop: jest.fn(() => callOrder.push('settlementExecutor')),
      };
      const mockChannelManager = {
        stop: jest.fn(() => callOrder.push('channelManager')),
      };
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (connectorNode as any)._settlementExecutor = mockSettlementExecutor;
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (connectorNode as any)._channelManager = mockChannelManager;

      // Act
      await connectorNode.stop();

      // Assert — executor must be stopped before channel manager
      expect(callOrder.indexOf('settlementExecutor')).toBeLessThan(
        callOrder.indexOf('channelManager')
      );
    });
  });

  describe('Deployment Mode Helpers', () => {
    beforeEach(() => {
      // Mock validateConfig to return the config (needed when constructing with config object)
      (ConfigLoader.validateConfig as jest.Mock) = jest
        .fn()
        .mockImplementation((cfg: ConnectorConfig) => cfg);
    });

    describe('getDeploymentMode()', () => {
      it('should return explicit deploymentMode when set to embedded', () => {
        // Arrange
        const config = createTestConfig({ deploymentMode: 'embedded' });
        const node = new ConnectorNode(config, mockLogger);

        // Act
        const mode = node.getDeploymentMode();

        // Assert
        expect(mode).toBe('embedded');
      });

      it('should return explicit deploymentMode when set to standalone', () => {
        // Arrange
        const config = createTestConfig({ deploymentMode: 'standalone' });
        const node = new ConnectorNode(config, mockLogger);

        // Act
        const mode = node.getDeploymentMode();

        // Assert
        expect(mode).toBe('standalone');
      });

      it('should infer standalone when localDelivery.enabled=true and adminApi.enabled=true', () => {
        // Arrange
        const config = createTestConfig({
          localDelivery: { enabled: true, handlerUrl: 'http://bls:8080' },
          adminApi: { enabled: true, port: 8081 },
        });
        const node = new ConnectorNode(config, mockLogger);

        // Act
        const mode = node.getDeploymentMode();

        // Assert
        expect(mode).toBe('standalone');
      });

      it('should infer embedded when localDelivery.enabled=false and adminApi.enabled=false', () => {
        // Arrange
        const config = createTestConfig({
          localDelivery: { enabled: false },
          adminApi: { enabled: false },
        });
        const node = new ConnectorNode(config, mockLogger);

        // Act
        const mode = node.getDeploymentMode();

        // Assert
        expect(mode).toBe('embedded');
      });

      it('should infer embedded when localDelivery and adminApi are not configured', () => {
        // Arrange
        const config = createTestConfig({});
        const node = new ConnectorNode(config, mockLogger);

        // Act
        const mode = node.getDeploymentMode();

        // Assert
        expect(mode).toBe('embedded');
      });

      it('should default to embedded for hybrid config (adminApi.enabled=true, localDelivery.enabled=false)', () => {
        // Arrange
        const config = createTestConfig({
          localDelivery: { enabled: false },
          adminApi: { enabled: true, port: 8081 },
        });
        const node = new ConnectorNode(config, mockLogger);

        // Act
        const mode = node.getDeploymentMode();

        // Assert
        expect(mode).toBe('embedded'); // Defaults to embedded for unusual configs
      });

      it('should default to embedded for hybrid config (adminApi.enabled=false, localDelivery.enabled=true)', () => {
        // Arrange
        const config = createTestConfig({
          localDelivery: { enabled: true, handlerUrl: 'http://bls:8080' },
          adminApi: { enabled: false },
        });
        const node = new ConnectorNode(config, mockLogger);

        // Act
        const mode = node.getDeploymentMode();

        // Assert
        expect(mode).toBe('embedded'); // Defaults to embedded for unusual configs
      });

      it('should prefer explicit deploymentMode over inferred mode', () => {
        // Arrange - explicit embedded but flags suggest standalone
        const config = createTestConfig({
          deploymentMode: 'embedded',
          localDelivery: { enabled: true, handlerUrl: 'http://bls:8080' },
          adminApi: { enabled: true, port: 8081 },
        });
        const node = new ConnectorNode(config, mockLogger);

        // Act
        const mode = node.getDeploymentMode();

        // Assert
        expect(mode).toBe('embedded'); // Explicit mode wins (validation will catch the conflict)
      });
    });

    describe('isEmbedded()', () => {
      it('should return true when deploymentMode is embedded', () => {
        // Arrange
        const config = createTestConfig({ deploymentMode: 'embedded' });
        const node = new ConnectorNode(config, mockLogger);

        // Act & Assert
        expect(node.isEmbedded()).toBe(true);
        expect(node.isStandalone()).toBe(false);
      });

      it('should return true when mode is inferred as embedded', () => {
        // Arrange
        const config = createTestConfig({
          localDelivery: { enabled: false },
          adminApi: { enabled: false },
        });
        const node = new ConnectorNode(config, mockLogger);

        // Act & Assert
        expect(node.isEmbedded()).toBe(true);
        expect(node.isStandalone()).toBe(false);
      });

      it('should return false when deploymentMode is standalone', () => {
        // Arrange
        const config = createTestConfig({ deploymentMode: 'standalone' });
        const node = new ConnectorNode(config, mockLogger);

        // Act & Assert
        expect(node.isEmbedded()).toBe(false);
      });
    });

    describe('isStandalone()', () => {
      it('should return true when deploymentMode is standalone', () => {
        // Arrange
        const config = createTestConfig({ deploymentMode: 'standalone' });
        const node = new ConnectorNode(config, mockLogger);

        // Act & Assert
        expect(node.isStandalone()).toBe(true);
        expect(node.isEmbedded()).toBe(false);
      });

      it('should return true when mode is inferred as standalone', () => {
        // Arrange
        const config = createTestConfig({
          localDelivery: { enabled: true, handlerUrl: 'http://bls:8080' },
          adminApi: { enabled: true, port: 8081 },
        });
        const node = new ConnectorNode(config, mockLogger);

        // Act & Assert
        expect(node.isStandalone()).toBe(true);
        expect(node.isEmbedded()).toBe(false);
      });

      it('should return false when deploymentMode is embedded', () => {
        // Arrange
        const config = createTestConfig({ deploymentMode: 'embedded' });
        const node = new ConnectorNode(config, mockLogger);

        // Act & Assert
        expect(node.isStandalone()).toBe(false);
      });
    });
  });
});
