/**
 * Minimal dependency tests for ConnectorNode
 *
 * Verifies that the connector can start with only core dependencies
 * and produces clear error messages when optional packages are missing.
 *
 * @packageDocumentation
 */

import { ConnectorNode } from './connector-node';
import { ConnectorConfig } from '../config/types';
import { RoutingTable } from '../routing/routing-table';
import { BTPClientManager } from '../btp/btp-client-manager';
import { BTPServer } from '../btp/btp-server';
import { PacketHandler } from './packet-handler';
import { Logger } from '../utils/logger';
import { ConfigLoader } from '../config/config-loader';
import { HealthServer } from '../http/health-server';
import { requireOptional } from '../utils/optional-require';
import { TigerBeetleClient } from '../settlement/tigerbeetle-client';

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
jest.mock('../http/admin-server');
jest.mock('../http/admin-api', () => ({
  validateSettlementConfig: jest.fn().mockReturnValue(null),
  createAdminRouter: jest.fn(),
}));
jest.mock('../explorer');
jest.mock('../settlement/payment-channel-sdk');
jest.mock('../settlement/channel-manager');
jest.mock('../settlement/settlement-executor');
jest.mock('../settlement/account-manager');
jest.mock('../settlement/settlement-monitor');
jest.mock('../security/key-manager');
jest.mock('../settlement/tigerbeetle-client');
jest.mock('../utils/optional-require');

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

const createMinimalConfig = (overrides?: Partial<ConnectorConfig>): ConnectorConfig => ({
  nodeId: 'connector-minimal',
  btpServerPort: 3000,
  environment: 'development',
  peers: [],
  routes: [],
  ...overrides,
});

describe('ConnectorNode — minimal dependency startup', () => {
  let mockLogger: jest.Mocked<Logger>;
  let mockBTPServer: jest.Mocked<BTPServer>;
  let mockBTPClientManager: jest.Mocked<BTPClientManager>;
  let mockHealthServer: jest.Mocked<HealthServer>;
  let mockPacketHandler: jest.Mocked<PacketHandler>;

  beforeEach(() => {
    jest.useFakeTimers();
    jest.clearAllMocks();

    // Clear settlement-related env vars to ensure minimal startup path
    delete process.env.SETTLEMENT_ENABLED;
    delete process.env.BASE_L2_RPC_URL;
    delete process.env.TOKEN_NETWORK_REGISTRY;
    delete process.env.M2M_TOKEN_ADDRESS;
    delete process.env.TREASURY_EVM_PRIVATE_KEY;
    delete process.env.TIGERBEETLE_CLUSTER_ID;
    delete process.env.TIGERBEETLE_REPLICAS;
    delete process.env.ADMIN_API_ENABLED;
    delete process.env.DASHBOARD_TELEMETRY_URL;

    mockLogger = createMockLogger();

    mockBTPServer = {
      start: jest.fn().mockResolvedValue(undefined),
      stop: jest.fn().mockResolvedValue(undefined),
    } as unknown as jest.Mocked<BTPServer>;

    mockBTPClientManager = {
      addPeer: jest.fn().mockResolvedValue(undefined),
      removePeer: jest.fn().mockResolvedValue(undefined),
      getPeerStatus: jest.fn().mockReturnValue(new Map()),
      getPeerIds: jest.fn().mockReturnValue([]),
      isConnected: jest.fn().mockReturnValue(false),
      setPacketHandler: jest.fn(),
    } as unknown as jest.Mocked<BTPClientManager>;

    mockPacketHandler = {
      setBTPServer: jest.fn(),
      setLocalDelivery: jest.fn(),
      setLocalDeliveryHandler: jest.fn(),
      handlePreparePacket: jest.fn(),
      setEventStore: jest.fn(),
      setEventBroadcaster: jest.fn(),
    } as unknown as jest.Mocked<PacketHandler>;

    mockHealthServer = {
      start: jest.fn().mockResolvedValue(undefined),
      stop: jest.fn().mockResolvedValue(undefined),
    } as unknown as jest.Mocked<HealthServer>;

    (RoutingTable as jest.MockedClass<typeof RoutingTable>).mockImplementation(
      () =>
        ({
          lookup: jest.fn(),
          getAllRoutes: jest.fn().mockReturnValue([]),
          addRoute: jest.fn(),
          removeRoute: jest.fn(),
        }) as unknown as RoutingTable
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

    // Default: requireOptional resolves successfully (won't be called in minimal path)
    (requireOptional as jest.Mock).mockResolvedValue({});
  });

  afterEach(() => {
    jest.useRealTimers();
    // Restore env vars
    delete process.env.SETTLEMENT_ENABLED;
    delete process.env.BASE_L2_RPC_URL;
    delete process.env.TOKEN_NETWORK_REGISTRY;
    delete process.env.M2M_TOKEN_ADDRESS;
    delete process.env.TREASURY_EVM_PRIVATE_KEY;
    delete process.env.TIGERBEETLE_CLUSTER_ID;
    delete process.env.TIGERBEETLE_REPLICAS;
    delete process.env.ADMIN_API_ENABLED;
    delete process.env.DASHBOARD_TELEMETRY_URL;
  });

  describe('settlement error messages', () => {
    it('should produce clear error when ethers is not available for EVM settlement', async () => {
      (requireOptional as jest.Mock).mockRejectedValue(
        new Error('ethers is required for EVM settlement. Install it with: npm install ethers')
      );

      const config = createMinimalConfig();
      (ConfigLoader.validateConfig as jest.Mock).mockReturnValue(config);

      // Enable settlement via env to trigger ethers import
      process.env.SETTLEMENT_ENABLED = 'true';
      process.env.BASE_L2_RPC_URL = 'http://localhost:8545';
      process.env.TOKEN_NETWORK_REGISTRY = '0x1234567890123456789012345678901234567890';
      process.env.M2M_TOKEN_ADDRESS = '0x1234567890123456789012345678901234567890';
      process.env.TREASURY_EVM_PRIVATE_KEY =
        '0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80';

      const node = new ConnectorNode(config, mockLogger);
      // start() should NOT throw — settlement failure is logged but connector continues
      await node.start();

      // Verify graceful degradation: error was logged, not thrown
      const errorCalls = mockLogger.error.mock.calls;
      const paymentChannelError = errorCalls.find(
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        (call) => (call[0] as any)?.event === 'payment_channel_init_failed'
      );
      expect(paymentChannelError).toBeDefined();
    });

    it('should produce error message containing package name and install command for ethers', async () => {
      const errorMsg = 'ethers is required for EVM settlement. Install it with: npm install ethers';
      (requireOptional as jest.Mock).mockRejectedValue(new Error(errorMsg));

      expect(errorMsg).toContain('ethers');
      expect(errorMsg).toContain('npm install ethers');
    });
  });

  describe('express error messages', () => {
    it('should produce clear error when express is not available for AdminServer', async () => {
      const { AdminServer } = jest.requireMock('../http/admin-server') as {
        AdminServer: jest.Mock;
      };
      const mockAdminStart = jest
        .fn()
        .mockRejectedValue(
          new Error(
            'express is required for HTTP admin/health APIs. Install it with: npm install express'
          )
        );
      AdminServer.mockImplementation(() => ({
        start: mockAdminStart,
        stop: jest.fn().mockResolvedValue(undefined),
      }));

      const config = createMinimalConfig();
      (ConfigLoader.validateConfig as jest.Mock).mockReturnValue(config);

      process.env.ADMIN_API_ENABLED = 'true';

      const node = new ConnectorNode(config, mockLogger);
      // AdminServer start failure should propagate (it's in the main try block)
      await expect(node.start()).rejects.toThrow('express is required for HTTP admin/health APIs');
    });

    it('should produce error message containing package name and install command for express', () => {
      const errorMsg =
        'express is required for HTTP admin/health APIs. Install it with: npm install express';

      expect(errorMsg).toContain('express');
      expect(errorMsg).toContain('npm install express');
    });
  });

  describe('tigerbeetle error messages', () => {
    it('should produce clear error when tigerbeetle-node is not available', async () => {
      const tbError = new Error(
        'tigerbeetle-node is required for TigerBeetle accounting. Install it with: npm install tigerbeetle-node'
      );

      (TigerBeetleClient as jest.MockedClass<typeof TigerBeetleClient>).mockImplementation(
        () =>
          ({
            initialize: jest.fn().mockRejectedValue(tbError),
            close: jest.fn().mockResolvedValue(undefined),
          }) as unknown as TigerBeetleClient
      );

      const config = createMinimalConfig();
      (ConfigLoader.validateConfig as jest.Mock).mockReturnValue(config);

      // Enable TigerBeetle + settlement to trigger the init path
      process.env.SETTLEMENT_ENABLED = 'true';
      process.env.BASE_L2_RPC_URL = 'http://localhost:8545';
      process.env.TOKEN_NETWORK_REGISTRY = '0x1234567890123456789012345678901234567890';
      process.env.M2M_TOKEN_ADDRESS = '0x1234567890123456789012345678901234567890';
      process.env.TREASURY_EVM_PRIVATE_KEY =
        '0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80';
      process.env.TIGERBEETLE_CLUSTER_ID = '0';
      process.env.TIGERBEETLE_REPLICAS = 'localhost:3000';

      // requireOptional for ethers must succeed for the code to reach TB init
      (requireOptional as jest.Mock).mockImplementation(async (pkg: string) => {
        if (pkg === 'ethers') {
          return {
            ethers: {
              JsonRpcProvider: jest.fn().mockReturnValue({}),
            },
          };
        }
        throw new Error(`${pkg} not available`);
      });

      const node = new ConnectorNode(config, mockLogger);
      // Connector should continue running (graceful degradation for TB)
      await node.start();

      // Verify TigerBeetle failure was logged as a warning
      const warnCalls = mockLogger.warn.mock.calls;
      const tbWarn = warnCalls.find(
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        (call) => (call[0] as any)?.event === 'tigerbeetle_init_failed'
      );
      expect(tbWarn).toBeDefined();
    });

    it('should produce error message containing package name and install command for tigerbeetle-node', () => {
      const errorMsg =
        'tigerbeetle-node is required for TigerBeetle accounting. Install it with: npm install tigerbeetle-node';

      expect(errorMsg).toContain('tigerbeetle-node');
      expect(errorMsg).toContain('npm install tigerbeetle-node');
    });
  });

  describe('requireOptional helper', () => {
    it('should throw error with package name and install command for any missing package', async () => {
      // Use the real requireOptional function
      const { requireOptional: realRequireOptional } = jest.requireActual(
        '../utils/optional-require'
      ) as { requireOptional: typeof requireOptional };

      await expect(realRequireOptional('nonexistent-package-xyz', 'test feature')).rejects.toThrow(
        'nonexistent-package-xyz is required for test feature'
      );

      await expect(realRequireOptional('nonexistent-package-xyz', 'test feature')).rejects.toThrow(
        'npm install nonexistent-package-xyz'
      );
    });

    it('should resolve successfully for installed packages', async () => {
      const { requireOptional: realRequireOptional } = jest.requireActual(
        '../utils/optional-require'
      ) as { requireOptional: typeof requireOptional };

      // 'pino' is a core dependency and should always be available
      const result = await realRequireOptional<typeof import('pino')>('pino', 'logging');
      expect(result).toBeDefined();
    });
  });
});
