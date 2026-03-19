/**
 * ConnectorNode - Core ILP connector orchestrator
 * Manages all connector components and lifecycle
 */

import { Logger } from '../utils/logger';
import { RoutingTable } from '../routing/routing-table';
import { BTPClientManager } from '../btp/btp-client-manager';
import { BTPServer } from '../btp/btp-server';
import { PacketHandler } from './packet-handler';
import { Peer } from '../btp/btp-client';
import {
  RoutingTableEntry,
  ILPAddress,
  isValidILPAddress,
  ILPPreparePacket,
  ILPFulfillPacket,
  ILPRejectPacket,
  PacketType,
  ILPErrorCode,
} from '@toon-protocol/shared';
import {
  ConnectorConfig,
  SettlementConfig,
  LocalDeliveryHandler,
  SendPacketParams,
  PeerRegistrationRequest,
  PeerInfo,
  PeerAccountBalance,
  RouteInfo,
  RemovePeerResult,
  DeploymentMode,
} from '../config/types';
import { PaymentHandler, createPaymentHandlerAdapter } from './payment-handler';
import {
  PeerConfig as SettlementPeerConfig,
  AdminSettlementConfig,
  normalizeChannelStatus,
} from '../settlement/types';
import { validateSettlementConfig } from '../http/admin-api';
import {
  ConfigLoader,
  ConfigurationError,
  ConnectorNotStartedError,
} from '../config/config-loader';
import { HealthServer } from '../http/health-server';
import { AdminServer } from '../http/admin-server';
import { HealthStatus, HealthStatusProvider } from '../http/types';
import { PaymentChannelSDK } from '../settlement/payment-channel-sdk';
import { ChannelManager } from '../settlement/channel-manager';
import { SettlementExecutor } from '../settlement/settlement-executor';
import { AccountManager } from '../settlement/account-manager';
import { SettlementMonitor } from '../settlement/settlement-monitor';
import { KeyManager } from '../security/key-manager';
import { requireOptional } from '../utils/optional-require';
import { TigerBeetleClient } from '../settlement/tigerbeetle-client';
import { InMemoryLedgerClient } from '../settlement/in-memory-ledger-client';
import { PerPacketClaimService } from '../settlement/per-packet-claim-service';
import {
  SENT_CLAIMS_TABLE_SCHEMA,
  SENT_CLAIMS_INDEXES,
} from '../settlement/claim-sender-db-schema';
import { InboundClaimValidator } from '../btp/inbound-claim-validator';
import { promises as dns } from 'dns';
// Import package.json for version information
import packageJson from '../../package.json';

/**
 * ConnectorNode - Main connector orchestrator
 * Coordinates RoutingTable, BTPClientManager, PacketHandler, and BTPServer
 * Implements connector startup, shutdown, and health monitoring
 */
export class ConnectorNode implements HealthStatusProvider {
  private readonly _config: ConnectorConfig;
  private readonly _logger: Logger;
  private readonly _routingTable: RoutingTable;
  private readonly _btpClientManager: BTPClientManager;
  private readonly _packetHandler: PacketHandler;
  private readonly _btpServer: BTPServer;
  private readonly _healthServer: HealthServer;
  private _adminServer: AdminServer | null = null;
  private _paymentChannelSDK: PaymentChannelSDK | null = null;
  private _chainSDKs: Map<number, PaymentChannelSDK> = new Map();
  private _channelManager: ChannelManager | null = null;
  private _accountManager: AccountManager | null = null;
  private _settlementMonitor: SettlementMonitor | null = null;
  private _settlementExecutor: SettlementExecutor | null = null;
  private _tigerBeetleClient: TigerBeetleClient | null = null;
  private _inMemoryLedgerClient: InMemoryLedgerClient | null = null;
  private readonly _settlementPeers: Map<string, SettlementPeerConfig> = new Map();
  private _healthStatus: 'healthy' | 'unhealthy' | 'starting' = 'starting';
  private readonly _startTime: Date = new Date();
  private _btpServerStarted: boolean = false;
  private _defaultSettlementTokenId: string = 'M2M';

  /**
   * The canonical token symbol resolved from the on-chain ERC-20 contract at startup.
   * Falls back to 'M2M' if the RPC call fails or settlement is disabled.
   */
  get defaultSettlementTokenId(): string {
    return this._defaultSettlementTokenId;
  }

  /**
   * Create ConnectorNode instance
   * @param config - ConnectorConfig object or path to YAML configuration file
   * @param logger - Pino logger instance
   * @throws ConfigurationError if configuration is invalid
   */
  constructor(config: ConnectorConfig | string, logger: Logger) {
    // Load and validate configuration
    let resolvedConfig: ConnectorConfig;
    try {
      if (typeof config === 'string') {
        resolvedConfig = ConfigLoader.loadConfig(config);
      } else {
        resolvedConfig = ConfigLoader.validateConfig(config);
      }
    } catch (error) {
      if (error instanceof ConfigurationError) {
        const logContext =
          typeof config === 'string'
            ? { event: 'config_load_failed', filePath: config, error: error.message }
            : { event: 'config_load_failed', source: 'object', error: error.message };
        logger.error(logContext, 'Failed to load configuration');
        throw error;
      }
      throw error;
    }

    this._config = resolvedConfig;
    this._logger = logger.child({ component: 'ConnectorNode', nodeId: resolvedConfig.nodeId });

    const loadedLogContext =
      typeof config === 'string'
        ? { event: 'config_loaded', filePath: config, nodeId: resolvedConfig.nodeId }
        : { event: 'config_loaded', source: 'object', nodeId: resolvedConfig.nodeId };
    this._logger.info(loadedLogContext, 'Configuration loaded successfully');

    // Convert RouteConfig[] to RoutingTableEntry[]
    const routingTableEntries: RoutingTableEntry[] = resolvedConfig.routes.map((route) => ({
      prefix: route.prefix as ILPAddress,
      nextHop: route.nextHop,
      priority: route.priority,
    }));

    // Initialize routing table
    this._routingTable = new RoutingTable(
      routingTableEntries,
      logger.child({ component: 'RoutingTable' })
    );

    // Initialize BTP client manager
    this._btpClientManager = new BTPClientManager(
      resolvedConfig.nodeId,
      logger.child({ component: 'BTPClientManager' })
    );

    // Initialize packet handler
    this._packetHandler = new PacketHandler(
      this._routingTable,
      this._btpClientManager,
      resolvedConfig.nodeId,
      logger.child({ component: 'PacketHandler' })
    );

    // Initialize BTP server
    this._btpServer = new BTPServer(logger.child({ component: 'BTPServer' }), this._packetHandler);

    // Link BTPServer to PacketHandler for bidirectional forwarding (resolves circular dependency)
    this._packetHandler.setBTPServer(this._btpServer);

    // Configure local delivery if enabled (forwards local packets to agent runtime)
    const localDeliveryEnabled =
      resolvedConfig.localDelivery?.enabled || process.env.LOCAL_DELIVERY_ENABLED === 'true';
    if (localDeliveryEnabled) {
      const localDeliveryConfig = {
        enabled: true,
        handlerUrl:
          resolvedConfig.localDelivery?.handlerUrl || process.env.LOCAL_DELIVERY_URL || '',
        timeout:
          resolvedConfig.localDelivery?.timeout ||
          parseInt(process.env.LOCAL_DELIVERY_TIMEOUT || '30000', 10),
        authToken: resolvedConfig.localDelivery?.authToken || process.env.LOCAL_DELIVERY_AUTH_TOKEN,
        perHopNotification:
          resolvedConfig.localDelivery?.perHopNotification ??
          process.env.LOCAL_DELIVERY_PER_HOP_NOTIFICATION === 'true',
      };
      this._packetHandler.setLocalDelivery(localDeliveryConfig);
    }

    // Link PacketHandler to BTPClientManager for incoming packet handling (resolves circular dependency)
    this._btpClientManager.setPacketHandler(this._packetHandler);

    // Initialize health server
    this._healthServer = new HealthServer(logger.child({ component: 'HealthServer' }), this);

    this._logger.info(
      {
        event: 'connector_initialized',
        nodeId: resolvedConfig.nodeId,
        peersCount: resolvedConfig.peers.length,
        routesCount: resolvedConfig.routes.length,
      },
      'Connector node initialized'
    );
  }

  /**
   * Register a direct in-process delivery handler for local ILP packets.
   * Bypasses the HTTP LocalDeliveryClient when set, delivering packets
   * directly to the handler function without an HTTP round-trip.
   *
   * @param handler - Function handler for local delivery, or null to clear and revert to HTTP fallback
   */
  setLocalDeliveryHandler(handler: LocalDeliveryHandler | null): void {
    this._logger.info(
      { event: 'local_delivery_handler_set', hasHandler: handler !== null },
      handler
        ? 'Local delivery function handler registered'
        : 'Local delivery function handler cleared'
    );
    this._packetHandler.setLocalDeliveryHandler(handler);
  }

  /**
   * Register a packet handler for local ILP packets.
   * Wraps the handler with an adapter that handles fulfillment computation,
   * error code mapping, and expiry checks — so the handler only needs to
   * return `{ accept: true }` or `{ accept: false }`.
   *
   * Shares the same underlying slot as `setLocalDeliveryHandler()` —
   * setting one overwrites the other (last writer wins).
   *
   * @param handler - Packet handler function, or null to clear
   */
  setPacketHandler(handler: PaymentHandler | null): void {
    this._logger.info(
      { event: 'packet_handler_set', hasHandler: handler !== null },
      handler ? 'Packet handler registered' : 'Packet handler cleared'
    );
    if (handler) {
      const adapter = createPaymentHandlerAdapter(handler, this._logger);
      this._packetHandler.setLocalDeliveryHandler(adapter);
    } else {
      this._packetHandler.setLocalDeliveryHandler(null);
    }
  }

  /**
   * Get the effective deployment mode for this connector.
   *
   * Returns the deployment mode based on configuration:
   * 1. If `config.deploymentMode` is explicitly set, returns that value
   * 2. Otherwise, infers mode from `localDelivery` and `adminApi` flags:
   *    - `localDelivery.enabled=true` + `adminApi.enabled=true` → 'standalone'
   *    - `localDelivery.enabled=false` + `adminApi.enabled=false` → 'embedded'
   *    - Other combinations → defaults to 'embedded'
   *
   * **Deployment Modes:**
   * - **embedded**: Connector runs in same process as business logic
   *   - Use `setPacketHandler()` or `setLocalDeliveryHandler()` for incoming packets
   *   - Use `node.sendPacket()` for outgoing packets
   *   - Admin API typically disabled
   *
   * - **standalone**: Connector runs as separate process/container
   *   - Incoming packets forwarded via HTTP to `/handle-packet` on external BLS
   *   - Outgoing packets sent via HTTP to `/admin/ilp/send` on connector admin API
   *   - Admin API enabled for external control
   *
   * @returns 'embedded' or 'standalone'
   *
   * @example
   * ```typescript
   * const mode = node.getDeploymentMode();
   * if (mode === 'embedded') {
   *   // In-process integration - use function handlers
   *   node.setPacketHandler(async (req) => ({ accept: true }));
   * } else {
   *   // Standalone mode - packets forwarded via HTTP
   *   console.log('Waiting for HTTP requests on /handle-packet');
   * }
   * ```
   */
  getDeploymentMode(): DeploymentMode {
    // Return explicit mode if configured
    if (this._config.deploymentMode) {
      return this._config.deploymentMode;
    }

    // Infer mode from configuration flags
    const hasLocalDelivery = this._config.localDelivery?.enabled === true;
    const hasAdminApi = this._config.adminApi?.enabled === true;

    // Standalone: Both HTTP delivery and admin API enabled
    if (hasLocalDelivery && hasAdminApi) {
      return 'standalone';
    }

    // Embedded: Both disabled (function handlers + library calls)
    if (!hasLocalDelivery && !hasAdminApi) {
      return 'embedded';
    }

    // Hybrid/unusual configuration — default to embedded
    // (e.g., adminApi enabled but localDelivery disabled = rare but valid)
    return 'embedded';
  }

  /**
   * Check if the connector is running in embedded mode.
   *
   * Embedded mode means the connector runs in the same process as business logic:
   * - Incoming packets handled via `setPacketHandler()` or `setLocalDeliveryHandler()`
   * - Outgoing packets sent via `node.sendPacket()` library calls
   * - Admin API typically disabled (not needed for in-process communication)
   * - Local delivery disabled (function handlers used instead of HTTP)
   *
   * @returns true if deployment mode is 'embedded', false otherwise
   *
   * @example
   * ```typescript
   * if (node.isEmbedded()) {
   *   node.setPacketHandler(async (req) => {
   *     console.log('Received packet:', req);
   *     return { accept: true };
   *   });
   * }
   * ```
   */
  isEmbedded(): boolean {
    return this.getDeploymentMode() === 'embedded';
  }

  /**
   * Check if the connector is running in standalone mode.
   *
   * Standalone mode means the connector runs as a separate process/container:
   * - Incoming packets forwarded via HTTP POST to `/handle-packet` on external BLS
   * - Outgoing packets sent via HTTP POST to `/admin/ilp/send` on connector admin API
   * - Admin API enabled for external control
   * - Local delivery enabled with `handlerUrl` pointing to external BLS
   *
   * @returns true if deployment mode is 'standalone', false otherwise
   *
   * @example
   * ```typescript
   * if (node.isStandalone()) {
   *   console.log('Connector running in standalone mode');
   *   console.log('Admin API:', node._config.adminApi?.port);
   *   console.log('BLS URL:', node._config.localDelivery?.handlerUrl);
   * }
   * ```
   */
  isStandalone(): boolean {
    return this.getDeploymentMode() === 'standalone';
  }

  /**
   * Send an ILP Prepare packet through the connector's routing logic.
   * Routes through PacketHandler using RoutingTable longest-prefix matching.
   *
   * @param params - Packet parameters (destination, amount, condition, expiry, data)
   * @returns ILP Fulfill or Reject packet
   * @throws ConnectorNotStartedError if connector has not been started
   */
  async sendPacket(params: SendPacketParams): Promise<ILPFulfillPacket | ILPRejectPacket> {
    if (!this._btpServerStarted) {
      throw new ConnectorNotStartedError();
    }

    const packet: ILPPreparePacket = {
      type: PacketType.PREPARE,
      destination: params.destination,
      amount: params.amount,
      executionCondition: params.executionCondition,
      expiresAt: params.expiresAt,
      data: params.data ?? Buffer.alloc(0),
    };

    this._logger.info(
      {
        event: 'send_packet',
        destination: params.destination,
        amount: params.amount.toString(),
        expiresAt: params.expiresAt.toISOString(),
      },
      'Sending packet via public API'
    );

    try {
      return await this._packetHandler.handlePreparePacket(packet, this._config.nodeId);
    } catch (error) {
      this._logger.error(
        {
          event: 'send_packet_error',
          destination: params.destination,
          error: error instanceof Error ? error.message : String(error),
        },
        'Unexpected error sending packet'
      );
      return {
        type: PacketType.REJECT,
        code: ILPErrorCode.T00_INTERNAL_ERROR,
        triggeredBy: this._config.nodeId,
        message: 'Internal connector error',
        data: Buffer.alloc(0),
      } as ILPRejectPacket;
    }
  }

  /**
   * Start connector and establish peer connections
   * Starts BTP server and connects to all configured peers
   */
  async start(): Promise<void> {
    this._logger.info(
      {
        event: 'connector_starting',
        nodeId: this._config.nodeId,
        peersCount: this._config.peers.length,
        routesCount: this._config.routes.length,
      },
      'Starting connector node'
    );

    try {
      // Initialize Base L2 Payment Channel infrastructure if enabled
      // Config-first pattern: settlementInfra config takes precedence, env var fallback
      const settlementEnabled =
        this._config.settlementInfra?.enabled ?? process.env.SETTLEMENT_ENABLED === 'true';
      const baseRpcUrl = this._config.settlementInfra?.rpcUrl ?? process.env.BASE_L2_RPC_URL;
      const registryAddress =
        this._config.settlementInfra?.registryAddress ?? process.env.TOKEN_NETWORK_REGISTRY;
      const m2mTokenAddress =
        this._config.settlementInfra?.tokenAddress ?? process.env.M2M_TOKEN_ADDRESS;
      const treasuryPrivateKey =
        this._config.settlementInfra?.privateKey ?? process.env.TREASURY_EVM_PRIVATE_KEY;

      if (
        settlementEnabled &&
        baseRpcUrl &&
        registryAddress &&
        m2mTokenAddress &&
        treasuryPrivateKey
      ) {
        try {
          // Initialize KeyManager with Environment backend using direct private key injection
          // No process.env mutation needed — enables multi-node isolation
          const keyManager = new KeyManager(
            {
              backend: 'env',
              nodeId: this._config.nodeId,
              evmPrivateKey: treasuryPrivateKey,
            },
            this._logger
          );

          // Use 'evm' as key ID (EnvironmentVariableBackend detects type from keyId)
          const evmKeyId = 'evm';

          // Initialize PaymentChannelSDK (primary chain)
          const { ethers } = await requireOptional<typeof import('ethers')>(
            'ethers',
            'EVM settlement'
          );
          const provider = new ethers.JsonRpcProvider(baseRpcUrl);
          this._paymentChannelSDK = new PaymentChannelSDK(
            provider,
            keyManager,
            evmKeyId,
            registryAddress,
            this._logger
          );

          // Resolve on-chain token symbol for canonical tokenId
          try {
            const resolvedSymbol = await this._paymentChannelSDK.getTokenSymbol(m2mTokenAddress);
            if (resolvedSymbol) {
              this._defaultSettlementTokenId = resolvedSymbol;
              this._logger.info(
                {
                  event: 'token_symbol_resolved',
                  symbol: resolvedSymbol,
                  tokenAddress: m2mTokenAddress,
                },
                `Resolved on-chain token symbol: ${resolvedSymbol}`
              );
            } else {
              this._logger.warn(
                { event: 'token_symbol_empty', tokenAddress: m2mTokenAddress },
                'ERC-20 symbol() returned empty string, falling back to M2M'
              );
            }
          } catch (symbolError) {
            this._logger.warn(
              {
                event: 'token_symbol_resolution_failed',
                tokenAddress: m2mTokenAddress,
                error: symbolError instanceof Error ? symbolError.message : String(symbolError),
              },
              'Failed to resolve on-chain token symbol, falling back to M2M'
            );
          }

          // Store primary SDK in chain map
          const primaryChainId =
            this._config.blockchain?.base?.chainId ?? this._config.blockchain?.arbitrum?.chainId;
          if (primaryChainId) {
            this._chainSDKs.set(primaryChainId, this._paymentChannelSDK);
          }

          // Initialize additional chain SDKs for multi-chain settlement
          const enabledChains: Array<{
            name: string;
            config: import('../config/types').EVMChainConfig;
          }> = [];
          if (this._config.blockchain?.base?.enabled && this._config.blockchain.base) {
            enabledChains.push({ name: 'Base', config: this._config.blockchain.base });
          }
          if (this._config.blockchain?.arbitrum?.enabled && this._config.blockchain.arbitrum) {
            enabledChains.push({ name: 'Arbitrum', config: this._config.blockchain.arbitrum });
          }

          for (const chain of enabledChains) {
            // Skip if already stored (primary chain)
            if (this._chainSDKs.has(chain.config.chainId)) {
              continue;
            }

            // Build per-chain config with settlementInfra fallbacks
            const chainRpcUrl = chain.config.rpcUrl;
            const chainRegistryAddress = chain.config.registryAddress ?? registryAddress;
            const chainPrivateKey = chain.config.privateKey ?? treasuryPrivateKey;

            // Create per-chain KeyManager if different private key
            const chainKeyManager =
              chainPrivateKey !== treasuryPrivateKey
                ? new KeyManager(
                    { backend: 'env', nodeId: this._config.nodeId, evmPrivateKey: chainPrivateKey },
                    this._logger
                  )
                : keyManager;

            const chainProvider = new ethers.JsonRpcProvider(chainRpcUrl);
            const chainSDK = new PaymentChannelSDK(
              chainProvider,
              chainKeyManager,
              evmKeyId,
              chainRegistryAddress,
              this._logger
            );
            this._chainSDKs.set(chain.config.chainId, chainSDK);

            this._logger.info(
              {
                event: 'chain_sdk_initialized',
                chain: chain.name,
                chainId: chain.config.chainId,
                rpcUrl: chainRpcUrl,
              },
              `PaymentChannelSDK initialized for ${chain.name} (chainId: ${chain.config.chainId})`
            );
          }

          // Build peer ID to EVM address mapping from config (with env var fallback)
          const peerIdToAddressMap = new Map<string, string>();
          for (const peer of this._config.peers) {
            if (peer.evmAddress) {
              peerIdToAddressMap.set(peer.id, peer.evmAddress);
              this._logger.debug(
                { peerId: peer.id, address: peer.evmAddress },
                'Loaded peer EVM address from config'
              );
            }
          }

          // Env var fallback for peers without evmAddress in config
          // Supports legacy PEER{N}_EVM_ADDRESS pattern (expanded to 10; will be removed in a future epic)
          for (let i = 1; i <= 10; i++) {
            const peerAddress = process.env[`PEER${i}_EVM_ADDRESS`];
            const peerId = `peer${i}`;
            if (peerAddress && !peerIdToAddressMap.has(peerId)) {
              peerIdToAddressMap.set(peerId, peerAddress);
              this._logger.debug(
                { peerId, address: peerAddress },
                'Loaded peer EVM address from env var (fallback)'
              );
            }
          }

          // Build token address map using the resolved on-chain symbol
          const tokenAddressMap = new Map<string, string>();
          tokenAddressMap.set(this._defaultSettlementTokenId, m2mTokenAddress);
          tokenAddressMap.set(m2mTokenAddress, m2mTokenAddress); // Also map address to itself for direct lookups

          // Initialize ChannelManager with TigerBeetle accounting if configured
          const defaultSettlementTimeout =
            this._config.settlementInfra?.settlementTimeoutSecs ?? 86400;
          const initialDepositMultiplier =
            this._config.settlementInfra?.initialDepositMultiplier ??
            parseInt(process.env.INITIAL_DEPOSIT_MULTIPLIER ?? '1', 10);

          // Initialize TigerBeetle AccountManager if configured (Story 19.1-19.2)
          // When TigerBeetle is unavailable, falls back to mock AccountManager (graceful degradation)
          let accountManager: AccountManager;
          const tigerBeetleClusterId = process.env.TIGERBEETLE_CLUSTER_ID;
          const tigerBeetleReplicas = process.env.TIGERBEETLE_REPLICAS;

          if (tigerBeetleClusterId && tigerBeetleReplicas) {
            try {
              // Resolve hostnames to IP addresses (TigerBeetle client requires IP addresses)
              const rawAddresses = tigerBeetleReplicas.split(',').map((s) => s.trim());
              const resolvedAddresses = await Promise.all(
                rawAddresses.map(async (addr) => {
                  const parts = addr.split(':');
                  const hostOrIp = parts[0] || addr;
                  const port = parts[1] || '3000';
                  // Check if already an IP address
                  if (/^\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(hostOrIp)) {
                    return addr;
                  }
                  // Resolve hostname to IP
                  try {
                    const result = await dns.lookup(hostOrIp);
                    this._logger.debug(
                      { hostname: hostOrIp, ip: result.address },
                      'Resolved TigerBeetle hostname to IP'
                    );
                    return `${result.address}:${port}`;
                  } catch (dnsError) {
                    this._logger.warn(
                      { hostname: hostOrIp, error: dnsError },
                      'Failed to resolve TigerBeetle hostname, using as-is'
                    );
                    return addr;
                  }
                })
              );

              // Create TigerBeetle client
              const tbOperationTimeout = parseInt(
                process.env.TIGERBEETLE_OPERATION_TIMEOUT ?? '15000',
                10
              );
              const tigerBeetleClient = new TigerBeetleClient(
                {
                  clusterId: parseInt(tigerBeetleClusterId, 10),
                  replicaAddresses: resolvedAddresses,
                  connectionTimeout: 5000,
                  operationTimeout: tbOperationTimeout,
                },
                this._logger
              );

              // Initialize TigerBeetle connection
              await tigerBeetleClient.initialize();
              this._tigerBeetleClient = tigerBeetleClient;

              // Create AccountManager
              accountManager = new AccountManager(
                { nodeId: this._config.nodeId },
                tigerBeetleClient,
                this._logger
              );

              this._accountManager = accountManager;

              this._logger.info(
                {
                  event: 'tigerbeetle_account_manager_initialized',
                  clusterId: tigerBeetleClusterId,
                  replicas: tigerBeetleReplicas,
                },
                `Accounting backend: TigerBeetle (cluster: ${tigerBeetleClusterId}, replicas: ${tigerBeetleReplicas})`
              );
            } catch (error) {
              // Fall back to in-memory ledger if TigerBeetle initialization fails
              const errorMessage = error instanceof Error ? error.message : String(error);
              this._logger.warn(
                {
                  event: 'tigerbeetle_init_failed',
                  error: errorMessage,
                  clusterId: tigerBeetleClusterId,
                  replicas: tigerBeetleReplicas,
                },
                'TigerBeetle initialization failed, using in-memory ledger'
              );
              // Create InMemoryLedgerClient-backed AccountManager
              accountManager = await this._createInMemoryAccountManager();
              this._accountManager = accountManager;
            }
          } else {
            this._logger.info(
              { event: 'tigerbeetle_not_configured' },
              'TigerBeetle not configured (TIGERBEETLE_CLUSTER_ID or TIGERBEETLE_REPLICAS not set), using in-memory ledger'
            );
            // Create InMemoryLedgerClient-backed AccountManager
            accountManager = await this._createInMemoryAccountManager();
            this._accountManager = accountManager;
          }

          // Initialize SettlementMonitor for threshold-based settlement triggering
          // Extract peer IDs from peerIdToAddressMap (includes all known peers in the network)
          const peerIds = Array.from(peerIdToAddressMap.keys());

          // Build settlement threshold configuration
          // Use settlementThreshold from config or default to 1M (1,000,000)
          // Note: threshold is typed as string in SettlementInfraConfig (YAML/JSON cannot represent BigInt)
          const settlementThreshold = BigInt(
            this._config.settlementInfra?.threshold ?? process.env.SETTLEMENT_THRESHOLD ?? '1000000'
          );
          const settlementPollingInterval =
            this._config.settlementInfra?.pollingIntervalMs ??
            parseInt(process.env.SETTLEMENT_POLLING_INTERVAL ?? '30000', 10);

          this._logger.info(
            {
              event: 'settlement_monitor_config',
              peerIds,
              threshold: settlementThreshold.toString(),
              pollingInterval: settlementPollingInterval,
            },
            'Initializing settlement monitor with peer list'
          );

          const settlementMonitor = new SettlementMonitor(
            {
              thresholds: {
                defaultThreshold: settlementThreshold,
                pollingInterval: settlementPollingInterval,
              },
              peers: peerIds,
              tokenIds: [this._defaultSettlementTokenId],
            },
            accountManager,
            this._logger
          );
          this._settlementMonitor = settlementMonitor;

          this._settlementExecutor = new SettlementExecutor(
            {
              nodeId: this._config.nodeId,
              defaultSettlementTimeout,
              initialDepositMultiplier,
              minDepositThreshold: 0.5,
              maxRetries: 3,
              retryDelayMs: 5000,
              tokenAddressMap,
              peerIdToAddressMap,
              registryAddress,
              rpcUrl: baseRpcUrl,
              privateKey: treasuryPrivateKey,
            },
            accountManager,
            this._paymentChannelSDK,
            settlementMonitor,
            this._logger
          );

          // Start automatic settlement execution
          this._settlementExecutor.start();
          this._logger.info(
            { event: 'settlement_executor_started' },
            'Automatic settlement execution enabled'
          );

          // Start monitoring after a short delay to ensure AccountManager is fully initialized
          setTimeout(async () => {
            try {
              await settlementMonitor.start();
              this._logger.info(
                {
                  event: 'settlement_monitor_started',
                  threshold: settlementThreshold.toString(),
                  peerCount: peerIds.length,
                  pollingInterval: settlementPollingInterval,
                },
                'Settlement threshold monitoring started'
              );
            } catch (error) {
              const errorMessage = error instanceof Error ? error.message : String(error);
              this._logger.error(
                { event: 'settlement_monitor_start_failed', error: errorMessage },
                'Failed to start settlement monitor'
              );
            }
          }, 5000); // 5 second delay

          this._channelManager = new ChannelManager(
            {
              nodeId: this._config.nodeId,
              defaultSettlementTimeout,
              initialDepositMultiplier,
              idleChannelThreshold: 86400,
              minDepositThreshold: 0.5,
              idleCheckInterval: 3600,
              tokenAddressMap,
              peerIdToAddressMap,
              registryAddress,
              rpcUrl: baseRpcUrl,
              privateKey: treasuryPrivateKey,
            },
            this._paymentChannelSDK,
            this._settlementExecutor,
            this._logger
          );

          this._logger.info(
            {
              event: 'payment_channel_sdk_initialized',
              registryAddress,
              tokenAddress: m2mTokenAddress,
              peerCount: peerIdToAddressMap.size,
            },
            'Payment channel infrastructure initialized'
          );

          // Wire PerPacketClaimService for attaching claims to outgoing packets
          if (this._channelManager && this._paymentChannelSDK) {
            try {
              const BetterSqlite3Module = await requireOptional<{
                default: new (path: string) => import('better-sqlite3').Database;
              }>('better-sqlite3', 'per-packet claims persistence');
              const BetterSqlite3 = BetterSqlite3Module.default;

              const claimDbPath = `./data/claims-${this._config.nodeId}.db`;
              const claimDb = new BetterSqlite3(claimDbPath);
              claimDb.exec(SENT_CLAIMS_TABLE_SCHEMA);
              for (const indexSql of SENT_CLAIMS_INDEXES) {
                claimDb.exec(indexSql);
              }

              const perPacketClaimService = new PerPacketClaimService(
                this._paymentChannelSDK,
                this._channelManager,
                claimDb,
                this._logger,
                this._config.nodeId
              );
              this._packetHandler.setPerPacketClaimService(perPacketClaimService);
              this._settlementExecutor?.setPerPacketClaimService(perPacketClaimService);

              this._logger.info(
                { event: 'per_packet_claims_enabled' },
                'Per-packet claim service wired to PacketHandler and SettlementExecutor'
              );
            } catch (error) {
              const errorMessage = error instanceof Error ? error.message : String(error);
              this._logger.error(
                { event: 'per_packet_claims_init_failed', error: errorMessage },
                'Failed to initialize per-packet claim service'
              );
              throw error;
            }
          }

          // Wire inbound claim validator to BTP server to prevent unpaid writes.
          // Every ILP PREPARE arriving via BTP must carry a valid signed claim
          // before reaching the packet handler / local delivery.
          const inboundClaimValidator = new InboundClaimValidator(
            this._paymentChannelSDK,
            this._config.nodeId,
            this._logger,
            this._channelManager ?? undefined
          );
          this._btpServer.setInboundClaimValidator((protocolData, ilpPacket, peerId) =>
            inboundClaimValidator.validate(protocolData, ilpPacket, peerId)
          );
          this._logger.info(
            { event: 'inbound_claim_validator_enabled' },
            'Inbound claim validator wired to BTP server'
          );

          // Wire AccountManager into PacketHandler for settlement recording
          if (accountManager) {
            const settlementConfig: SettlementConfig = {
              connectorFeePercentage: this._config.settlement?.connectorFeePercentage ?? 0.1,
              enableSettlement: settlementEnabled,
              tigerBeetleClusterId: tigerBeetleClusterId ? parseInt(tigerBeetleClusterId, 10) : 0,
              tigerBeetleReplicas: tigerBeetleReplicas
                ? tigerBeetleReplicas.split(',').map((s) => s.trim())
                : [],
            };

            this._packetHandler.setSettlement(
              accountManager,
              settlementConfig,
              this._defaultSettlementTokenId
            );
          }
        } catch (error) {
          // Log error but continue without payment channels (graceful degradation)
          const errorMessage = error instanceof Error ? error.message : String(error);
          this._logger.error(
            { event: 'payment_channel_init_failed', error: errorMessage },
            'Failed to initialize payment channel infrastructure (connector continues without channels)'
          );
        }
      } else {
        this._logger.info(
          { event: 'payment_channels_disabled' },
          'Payment channel infrastructure disabled (missing configuration)'
        );
      }

      // Start BTP server to accept incoming connections
      await this._btpServer.start(this._config.btpServerPort);
      this._btpServerStarted = true;
      this._logger.info(
        {
          event: 'btp_server_started',
          port: this._config.btpServerPort,
        },
        'BTP server started'
      );

      // Start health server
      const healthCheckPort = this._config.healthCheckPort || 8080;
      await this._healthServer.start(healthCheckPort);
      this._logger.info(
        {
          event: 'health_server_started',
          port: healthCheckPort,
        },
        'Health server started'
      );

      // Start admin API server if enabled
      const adminApiEnabled =
        this._config.adminApi?.enabled || process.env.ADMIN_API_ENABLED === 'true';
      if (adminApiEnabled) {
        const adminConfig = {
          enabled: true,
          port: this._config.adminApi?.port ?? parseInt(process.env.ADMIN_API_PORT || '8081', 10),
          host: this._config.adminApi?.host ?? process.env.ADMIN_API_HOST ?? '0.0.0.0',
          apiKey: this._config.adminApi?.apiKey ?? process.env.ADMIN_API_KEY,
        };

        this._adminServer = new AdminServer({
          routingTable: this._routingTable,
          btpClientManager: this._btpClientManager,
          nodeId: this._config.nodeId,
          config: adminConfig,
          logger: this._logger,
          settlementPeers: this._settlementPeers,
          channelManager: this._channelManager ?? undefined,
          paymentChannelSDK: this._paymentChannelSDK ?? undefined,
          accountManager: this._accountManager ?? undefined,
          settlementMonitor: this._settlementMonitor ?? undefined,
          defaultSettlementTokenId: this._defaultSettlementTokenId,
          packetSender: (params) => this.sendPacket(params),
          isReady: () => this._btpServerStarted,
        });

        await this._adminServer.start();
        this._logger.info(
          {
            event: 'admin_server_started',
            port: adminConfig.port,
            host: adminConfig.host,
            apiKeyConfigured: !!adminConfig.apiKey,
          },
          'Admin API server started'
        );
      } else {
        this._logger.debug(
          { event: 'admin_api_disabled' },
          'Admin API disabled (set ADMIN_API_ENABLED=true or adminApi.enabled=true to enable)'
        );
      }

      // Connect BTP clients to all configured peers
      // Convert PeerConfig to Peer format
      const peerConnections: Promise<void>[] = [];
      for (const peerConfig of this._config.peers) {
        const peer: Peer = {
          id: peerConfig.id,
          url: peerConfig.url,
          authToken: peerConfig.authToken,
          connected: false,
          lastSeen: new Date(),
        };
        peerConnections.push(this._btpClientManager.addPeer(peer));
      }

      // Wait for all peer connection attempts (don't fail if some connections fail)
      // BTPClient will automatically retry failed connections in the background
      const peerResults = await Promise.allSettled(peerConnections);
      const failedPeers = peerResults.filter((r) => r.status === 'rejected');
      if (failedPeers.length > 0) {
        this._logger.warn(
          {
            event: 'peer_connection_failures',
            failedCount: failedPeers.length,
            totalPeers: this._config.peers.length,
          },
          'Some peer connections failed during startup (will retry in background)'
        );
      }

      const connectedPeers = this._btpClientManager.getPeerStatus();
      const connectedCount = Array.from(connectedPeers.values()).filter(Boolean).length;

      // Create payment channels for connected peers (if channel infrastructure is enabled)
      if (this._channelManager && this._paymentChannelSDK) {
        this._logger.info(
          { event: 'creating_payment_channels', connectedCount },
          'Creating payment channels for connected peers'
        );

        const channelCreationPromises: Promise<void>[] = [];
        for (const [peerId, connected] of connectedPeers.entries()) {
          if (!connected) {
            continue; // Skip disconnected peers
          }

          // Create channel creation promise (don't await - run in parallel)
          const channelPromise = (async () => {
            try {
              const tokenId = this._defaultSettlementTokenId;
              const channelId = await this._channelManager!.ensureChannelExists(peerId, tokenId);
              this._logger.info(
                { event: 'payment_channel_ready', peerId, channelId },
                'Payment channel ready for peer'
              );
            } catch (error) {
              // Don't fail startup if channel creation fails
              const errorMessage = error instanceof Error ? error.message : String(error);
              this._logger.warn(
                { event: 'payment_channel_creation_failed', peerId, error: errorMessage },
                'Failed to create payment channel for peer (will retry on-demand)'
              );
            }
          })();

          channelCreationPromises.push(channelPromise);
        }

        // Wait for all channel creation attempts (but don't fail if some fail)
        await Promise.allSettled(channelCreationPromises);
        this._logger.info(
          { event: 'payment_channels_initialized' },
          'Payment channel creation completed'
        );
      }

      // Update health status to healthy after all components started
      this._updateHealthStatus();

      this._logger.info(
        {
          event: 'connector_ready',
          nodeId: this._config.nodeId,
          connectedPeers: connectedCount,
          totalPeers: this._config.peers.length,
          healthStatus: this._healthStatus,
        },
        'Connector node ready'
      );
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : String(error);
      this._logger.error(
        {
          event: 'connector_start_failed',
          nodeId: this._config.nodeId,
          error: errorMessage,
        },
        'Failed to start connector node'
      );
      this._healthStatus = 'unhealthy';
      throw error;
    }
  }

  /**
   * Stop connector and disconnect all peers
   * Gracefully shuts down all components
   */
  async stop(): Promise<void> {
    // Idempotent guard: if already stopped, return immediately
    if (!this._btpServerStarted && !this._adminServer) {
      this._logger.debug(
        { event: 'connector_already_stopped' },
        'Connector already stopped, ignoring'
      );
      return;
    }

    this._logger.info(
      {
        event: 'connector_stopping',
        nodeId: this._config.nodeId,
      },
      'Stopping connector node'
    );

    try {
      // Stop settlement monitor FIRST to stop polling the ledger during drain.
      // The executor already unsubscribes in its own stop(), so no new events fire.
      if (this._settlementMonitor) {
        await this._settlementMonitor.stop();
        this._logger.info({ event: 'settlement_monitor_stopped' }, 'Settlement monitor stopped');
        this._settlementMonitor = null;
      }

      // Stop settlement executor — awaits in-flight settlements to prevent
      // on-chain/off-chain balance mismatches on SIGTERM/shutdown
      if (this._settlementExecutor) {
        await this._settlementExecutor.stop();
        this._logger.info({ event: 'settlement_executor_stopped' }, 'Settlement executor stopped');
        this._settlementExecutor = null;
      }

      // Stop channel manager if running
      if (this._channelManager) {
        this._channelManager.stop();
        this._logger.info({ event: 'channel_manager_stopped' }, 'Channel manager stopped');
        this._channelManager = null;
      }

      // Clean up all chain SDKs
      for (const [chainId, sdk] of this._chainSDKs.entries()) {
        sdk.removeAllListeners();
        this._logger.debug(
          { event: 'chain_sdk_stopped', chainId },
          `Chain SDK stopped (chainId: ${chainId})`
        );
      }
      this._chainSDKs.clear();

      // Clean up primary payment channel SDK reference
      if (this._paymentChannelSDK) {
        // Already cleaned up via _chainSDKs iteration above, just null the reference
        this._logger.info({ event: 'payment_channel_sdk_stopped' }, 'Payment channel SDK stopped');
        this._paymentChannelSDK = null;
      }

      // Close TigerBeetle client if connected
      if (this._tigerBeetleClient) {
        await this._tigerBeetleClient.close();
        this._logger.info({ event: 'tigerbeetle_client_closed' }, 'TigerBeetle client closed');
        this._tigerBeetleClient = null;
      }

      // Close InMemoryLedgerClient if connected (ensures final snapshot persistence)
      if (this._inMemoryLedgerClient) {
        await this._inMemoryLedgerClient.close();
        this._logger.info({ event: 'in_memory_ledger_closed' }, 'In-memory ledger client closed');
        this._inMemoryLedgerClient = null;
      }

      this._accountManager = null;

      // Disconnect all BTP clients
      const peerIds = this._btpClientManager.getPeerIds();
      for (const peerId of peerIds) {
        await this._btpClientManager.removePeer(peerId);
      }

      // Stop admin server if running
      if (this._adminServer) {
        await this._adminServer.stop();
        this._logger.info({ event: 'admin_server_stopped' }, 'Admin API server stopped');
        this._adminServer = null;
      }

      // Stop health server
      await this._healthServer.stop();

      // Stop BTP server
      await this._btpServer.stop();

      this._logger.info(
        {
          event: 'connector_stopped',
          nodeId: this._config.nodeId,
        },
        'Connector node stopped'
      );

      this._healthStatus = 'starting'; // Reset to initial state
      this._btpServerStarted = false;
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : String(error);
      this._logger.error(
        {
          event: 'connector_stop_failed',
          nodeId: this._config.nodeId,
          error: errorMessage,
        },
        'Failed to stop connector node gracefully'
      );
      throw error;
    }
  }

  /**
   * Get connector health status (implements HealthStatusProvider interface)
   * @returns Current health status including connected peers and uptime
   */
  getHealthStatus(): HealthStatus {
    const peerStatus = this._btpClientManager.getPeerStatus();
    const peersConnected = Array.from(peerStatus.values()).filter(Boolean).length;
    const totalPeers = this._config.peers.length;
    const uptime = Math.floor((Date.now() - this._startTime.getTime()) / 1000);

    const healthStatus: HealthStatus = {
      status: this._healthStatus,
      uptime,
      peersConnected,
      totalPeers,
      timestamp: new Date().toISOString(),
      nodeId: this._config.nodeId,
      version: packageJson.version,
    };

    return healthStatus;
  }

  /**
   * Update health status based on current peer connections
   * Called internally when connection state changes
   * @private
   */

  /**
   * Get routing table instance (for admin API access)
   * @returns RoutingTable instance
   */
  get routingTable(): RoutingTable {
    return this._routingTable;
  }

  /**
   * Get BTP client manager instance (for admin API access)
   * @returns BTPClientManager instance
   */
  get btpClientManager(): BTPClientManager {
    return this._btpClientManager;
  }

  /**
   * Get payment channel SDK instance (for admin API access)
   * @returns PaymentChannelSDK instance or null if not initialized
   */
  get paymentChannelSDK(): PaymentChannelSDK | null {
    return this._paymentChannelSDK;
  }

  /**
   * Get channel manager instance (for admin API access)
   * @returns ChannelManager instance or null if not initialized
   */
  get channelManager(): ChannelManager | null {
    return this._channelManager;
  }

  /**
   * Get account manager instance (for admin API access)
   * @returns AccountManager instance or null if not initialized
   */
  get accountManager(): AccountManager | null {
    return this._accountManager;
  }

  /**
   * Get PaymentChannelSDK for a specific chain ID.
   * Used for multi-chain settlement when peers settle on different chains.
   *
   * @param chainId - EVM chain ID (e.g., 8453 for Base, 42161 for Arbitrum)
   * @returns PaymentChannelSDK for the chain, or null if not initialized
   */
  getPaymentChannelSDKForChain(chainId: number): PaymentChannelSDK | null {
    return this._chainSDKs.get(chainId) ?? null;
  }

  /**
   * Creates an AccountManager backed by InMemoryLedgerClient when TigerBeetle is unavailable.
   * Provides working balance tracking with snapshot persistence.
   * @returns AccountManager instance with in-memory ledger backend
   * @private
   */
  private async _createInMemoryAccountManager(): Promise<AccountManager> {
    const snapshotPath =
      this._config.settlementInfra?.ledgerSnapshotPath ??
      process.env.LEDGER_SNAPSHOT_PATH ??
      './data/ledger-snapshot.json';
    const persistIntervalMs =
      this._config.settlementInfra?.ledgerPersistIntervalMs ??
      parseInt(process.env.LEDGER_PERSIST_INTERVAL_MS || '30000', 10);

    let inMemoryClient: InMemoryLedgerClient;

    try {
      // Create InMemoryLedgerClient with persistence config
      inMemoryClient = new InMemoryLedgerClient(
        {
          snapshotPath,
          persistIntervalMs,
        },
        this._logger
      );

      // Initialize (will restore from snapshot if it exists)
      await inMemoryClient.initialize();

      this._logger.info(
        {
          event: 'in_memory_ledger_initialized',
          snapshotPath,
          persistIntervalMs,
        },
        `Accounting backend: in-memory ledger (snapshot: ${snapshotPath})`
      );
    } catch (error) {
      // Snapshot restore failed (corrupt file, disk permission, etc.)
      // Retry with fresh in-memory client (no snapshot restore)
      const errorMessage = error instanceof Error ? error.message : String(error);
      this._logger.warn(
        {
          event: 'in_memory_ledger_snapshot_restore_failed',
          error: errorMessage,
          snapshotPath,
        },
        'Failed to restore from snapshot, starting with fresh in-memory ledger'
      );

      try {
        // Create fresh client with a unique path to skip snapshot restore
        inMemoryClient = new InMemoryLedgerClient(
          {
            snapshotPath: `${snapshotPath}.fresh-${Date.now()}`,
            persistIntervalMs,
          },
          this._logger
        );

        await inMemoryClient.initialize();

        this._logger.info(
          {
            event: 'in_memory_ledger_fresh_start',
            snapshotPath,
          },
          'In-memory ledger started with empty state'
        );
      } catch (freshInitError) {
        // Even fresh initialization failed - this should be impossible
        // Re-throw to let outer settlement block catch handle it
        const freshErrorMessage =
          freshInitError instanceof Error ? freshInitError.message : String(freshInitError);
        this._logger.error(
          {
            event: 'in_memory_ledger_fresh_init_failed',
            error: freshErrorMessage,
          },
          'Critical: Fresh in-memory ledger initialization failed'
        );
        throw freshInitError;
      }
    }

    // Store reference for shutdown lifecycle
    this._inMemoryLedgerClient = inMemoryClient;

    const accountManager = new AccountManager(
      { nodeId: this._config.nodeId },
      inMemoryClient,
      this._logger
    );

    return accountManager;
  }

  private _updateHealthStatus(): void {
    // During startup phase (BTP server not listening yet)
    if (!this._btpServerStarted) {
      if (this._healthStatus !== 'starting') {
        this._logger.info(
          {
            event: 'health_status_changed',
            oldStatus: this._healthStatus,
            newStatus: 'starting',
            reason: 'BTP server not started',
          },
          'Health status changed'
        );
        this._healthStatus = 'starting';
      }
      return;
    }

    // If no peers configured, connector is healthy (standalone mode)
    const totalPeers = this._config.peers.length;
    if (totalPeers === 0) {
      if (this._healthStatus !== 'healthy') {
        this._logger.info(
          {
            event: 'health_status_changed',
            oldStatus: this._healthStatus,
            newStatus: 'healthy',
            reason: 'No peers configured (standalone mode)',
          },
          'Health status changed'
        );
        this._healthStatus = 'healthy';
      }
      return;
    }

    // Calculate connection percentage
    const peerStatus = this._btpClientManager.getPeerStatus();
    const connectedCount = Array.from(peerStatus.values()).filter(Boolean).length;
    const connectionPercentage = (connectedCount / totalPeers) * 100;

    // Determine new health status
    let newStatus: 'healthy' | 'unhealthy' | 'starting';
    let reason: string;

    if (connectionPercentage < 50) {
      newStatus = 'unhealthy';
      reason = `Only ${connectedCount}/${totalPeers} peers connected (<50%)`;
    } else {
      newStatus = 'healthy';
      reason = `${connectedCount}/${totalPeers} peers connected (≥50%)`;
    }

    // Log status changes
    if (this._healthStatus !== newStatus) {
      this._logger.info(
        { event: 'health_status_changed', oldStatus: this._healthStatus, newStatus, reason },
        'Health status changed'
      );
      this._healthStatus = newStatus;
    }
  }

  // ────────────────────────────────────────────────────────────────────────────
  // Admin Operations — direct method API (Story 24.4)
  // ────────────────────────────────────────────────────────────────────────────

  /**
   * Register a new peer with BTP connection and optional routes/settlement config.
   * Equivalent to POST /admin/peers — same validation and behavior.
   *
   * @param config - Peer registration parameters
   * @returns PeerInfo with connection status
   * @throws ConnectorNotStartedError if connector has not been started
   * @throws Error('Missing or invalid peer id') if id is missing/empty
   * @throws Error('URL must start with ws:// or wss://') if url format is invalid
   * @throws Error('Invalid ILP address prefix: ...') if route prefix is invalid
   * @throws Error (from validateSettlementConfig) if settlement config is invalid
   */
  async registerPeer(config: PeerRegistrationRequest): Promise<PeerInfo> {
    if (!this._btpServerStarted) {
      throw new ConnectorNotStartedError(
        'Connector is not started. Call start() before registerPeer().'
      );
    }

    // Validate required fields
    if (!config.id || typeof config.id !== 'string') {
      throw new Error('Missing or invalid peer id');
    }
    if (!config.url || typeof config.url !== 'string') {
      throw new Error('Missing or invalid peer url');
    }
    if (
      config.authToken === undefined ||
      config.authToken === null ||
      typeof config.authToken !== 'string'
    ) {
      throw new Error('authToken must be a string (can be empty for no auth)');
    }

    // Validate URL format
    if (!config.url.startsWith('ws://') && !config.url.startsWith('wss://')) {
      throw new Error('URL must start with ws:// or wss://');
    }

    // Validate routes if provided
    if (config.routes) {
      for (const route of config.routes) {
        if (!route.prefix || typeof route.prefix !== 'string') {
          throw new Error('Invalid route: missing prefix');
        }
        if (!isValidILPAddress(route.prefix)) {
          throw new Error(`Invalid ILP address prefix: ${route.prefix}`);
        }
      }
    }

    // Validate settlement config if provided
    if (config.settlement) {
      const settlementError = validateSettlementConfig(config.settlement);
      if (settlementError) {
        throw new Error(settlementError);
      }
    }

    // Check if peer already exists (idempotent re-registration)
    const existingPeers = this._btpClientManager.getPeerIds();
    const isUpdate = existingPeers.includes(config.id);

    // Only add BTP peer on initial registration
    if (!isUpdate) {
      const peer: Peer = {
        id: config.id,
        url: config.url,
        authToken: config.authToken,
        connected: false,
        lastSeen: new Date(),
      };
      await this._btpClientManager.addPeer(peer);
      this._logger.info(
        { event: 'peer_registered', peerId: config.id, url: config.url },
        `Registered peer: ${config.id}`
      );
    } else {
      this._logger.info(
        { event: 'peer_reregistered', peerId: config.id },
        `Re-registering peer: ${config.id}`
      );
    }

    // Add routes if provided
    if (config.routes) {
      for (const route of config.routes) {
        this._routingTable.addRoute(route.prefix as ILPAddress, config.id, route.priority ?? 0);
        this._logger.info(
          { event: 'route_added', prefix: route.prefix, nextHop: config.id },
          `Added route: ${route.prefix} -> ${config.id}`
        );
      }
    }

    // Create/merge settlement config
    if (config.settlement) {
      this._applySettlementConfig(config.id, config.settlement, config.routes, isUpdate);
    }

    // Build PeerInfo response
    const routes = this._routingTable.getAllRoutes();
    const peerRoutes = routes.filter((r) => r.nextHop === config.id);
    const connected = this._btpClientManager.isConnected(config.id);

    const peerInfo: PeerInfo = {
      id: config.id,
      connected,
      ilpAddresses: peerRoutes.map((r) => r.prefix),
      routeCount: peerRoutes.length,
    };

    const peerConfig = this._settlementPeers.get(config.id);
    if (peerConfig) {
      peerInfo.settlement = {
        preference: peerConfig.settlementPreference,
        evmAddress: peerConfig.evmAddress,
        tokenAddress: peerConfig.tokenAddress,
        chainId: peerConfig.chainId,
      };
    }

    return peerInfo;
  }

  /**
   * Remove a peer, disconnect BTP connection, and optionally remove associated routes.
   * Equivalent to DELETE /admin/peers/:peerId — same validation and behavior.
   *
   * @param peerId - Peer identifier to remove
   * @param removeRoutes - Whether to remove routes associated with this peer (default: true)
   * @returns RemovePeerResult with peerId and list of removed route prefixes
   * @throws ConnectorNotStartedError if connector has not been started
   * @throws Error('Peer not found: ...') if peer does not exist
   */
  async removePeer(peerId: string, removeRoutes: boolean = true): Promise<RemovePeerResult> {
    if (!this._btpServerStarted) {
      throw new ConnectorNotStartedError(
        'Connector is not started. Call start() before removePeer().'
      );
    }

    // Check peer exists
    const existingPeers = this._btpClientManager.getPeerIds();
    if (!existingPeers.includes(peerId)) {
      throw new Error(`Peer not found: ${peerId}`);
    }

    // Remove BTP peer
    await this._btpClientManager.removePeer(peerId);
    this._logger.info({ event: 'peer_removed', peerId }, `Removed peer: ${peerId}`);

    // Remove settlement config
    if (this._settlementPeers.delete(peerId)) {
      this._logger.info(
        { event: 'settlement_config_removed', peerId },
        `Removed settlement config for peer: ${peerId}`
      );
    }

    // Remove routes if requested
    const removedRoutes: string[] = [];
    if (removeRoutes) {
      const routes = this._routingTable.getAllRoutes();
      for (const route of routes) {
        if (route.nextHop === peerId) {
          this._routingTable.removeRoute(route.prefix);
          removedRoutes.push(route.prefix);
          this._logger.info(
            { event: 'route_removed', prefix: route.prefix },
            `Removed route: ${route.prefix}`
          );
        }
      }
    }

    return { peerId, removedRoutes };
  }

  /**
   * List all peers with connection status and routing info.
   * Equivalent to GET /admin/peers — same response shape.
   *
   * @returns Array of PeerInfo objects
   */
  listPeers(): PeerInfo[] {
    const peerIds = this._btpClientManager.getPeerIds();
    const peerStatus = this._btpClientManager.getPeerStatus();
    const routes = this._routingTable.getAllRoutes();

    return peerIds.map((peerId) => {
      const peerRoutes = routes.filter((r) => r.nextHop === peerId);
      const peerInfo: PeerInfo = {
        id: peerId,
        connected: peerStatus.get(peerId) ?? false,
        ilpAddresses: peerRoutes.map((r) => r.prefix),
        routeCount: peerRoutes.length,
      };

      const peerConfig = this._settlementPeers.get(peerId);
      if (peerConfig) {
        peerInfo.settlement = {
          preference: peerConfig.settlementPreference,
          evmAddress: peerConfig.evmAddress,
          tokenAddress: peerConfig.tokenAddress,
          chainId: peerConfig.chainId,
        };
      }

      return peerInfo;
    });
  }

  /**
   * Get balance for a specific peer from TigerBeetle.
   * Equivalent to GET /admin/balances/:peerId — same response shape.
   *
   * @param peerId - Peer identifier
   * @param tokenId - Token identifier (defaults to the resolved on-chain symbol, e.g. 'M2M')
   * @returns PeerAccountBalance with debit/credit/net balances
   * @throws Error if account management is not enabled
   */
  async getBalance(
    peerId: string,
    tokenId: string = this._defaultSettlementTokenId
  ): Promise<PeerAccountBalance> {
    if (!this._accountManager) {
      throw new Error('Account management not enabled');
    }

    const balance = await this._accountManager.getAccountBalance(peerId, tokenId);
    return {
      peerId,
      balances: [
        {
          tokenId,
          debitBalance: balance.debitBalance.toString(),
          creditBalance: balance.creditBalance.toString(),
          netBalance: balance.netBalance.toString(),
        },
      ],
    };
  }

  /**
   * List all routes in the routing table.
   * Equivalent to GET /admin/routes — same response shape.
   *
   * @returns Array of RouteInfo objects
   */
  listRoutes(): RouteInfo[] {
    const routes = this._routingTable.getAllRoutes();
    return routes.map((r) => ({
      prefix: r.prefix,
      nextHop: r.nextHop,
      priority: r.priority ?? 0,
    }));
  }

  /**
   * Add a static route to the routing table.
   * Equivalent to POST /admin/routes — same validation.
   *
   * @param route - Route configuration (prefix, nextHop, priority)
   * @throws Error('Invalid ILP address prefix: ...') if prefix is not a valid ILP address
   * @throws Error('Missing or invalid nextHop') if nextHop is empty
   */
  addRoute(route: RouteInfo): void {
    // Validate prefix
    if (!isValidILPAddress(route.prefix)) {
      throw new Error(`Invalid ILP address prefix: ${route.prefix}`);
    }

    // Validate nextHop
    if (!route.nextHop || typeof route.nextHop !== 'string') {
      throw new Error('Missing or invalid nextHop');
    }

    // Warn if nextHop peer doesn't exist (but don't block)
    const existingPeers = this._btpClientManager.getPeerIds();
    if (!existingPeers.includes(route.nextHop)) {
      this._logger.warn(
        { event: 'route_nextHop_unknown', prefix: route.prefix, nextHop: route.nextHop },
        `Adding route with unknown nextHop peer: ${route.nextHop}`
      );
    }

    this._routingTable.addRoute(route.prefix as ILPAddress, route.nextHop, route.priority ?? 0);

    this._logger.info(
      { event: 'route_added', prefix: route.prefix, nextHop: route.nextHop },
      `Added route: ${route.prefix} -> ${route.nextHop}`
    );
  }

  /**
   * Remove a route from the routing table by prefix.
   * Equivalent to DELETE /admin/routes/:prefix — same validation.
   *
   * @param prefix - ILP address prefix of the route to remove
   * @throws Error('Route not found: ...') if no route with the given prefix exists
   */
  removeRoute(prefix: string): void {
    const routes = this._routingTable.getAllRoutes();
    const exists = routes.some((r) => r.prefix === prefix);
    if (!exists) {
      throw new Error(`Route not found: ${prefix}`);
    }

    this._routingTable.removeRoute(prefix as ILPAddress);
    this._logger.info({ event: 'route_removed', prefix }, `Removed route: ${prefix}`);
  }

  // ────────────────────────────────────────────────────────────────────────────
  // Payment Channel Operations — direct method API
  // ────────────────────────────────────────────────────────────────────────────

  /**
   * Open a payment channel for a registered peer.
   * Equivalent to POST /admin/channels (EVM path) — same validation and behavior.
   *
   * @param params - Channel open parameters
   * @returns Object with channelId and normalized status
   * @throws ConnectorNotStartedError if connector has not been started
   * @throws Error('Settlement infrastructure not enabled') if channelManager is null
   * @throws Error('Peer ... must be registered before opening channels') if peer not found
   * @throws Error('Channel already exists for peer ...') if active channel exists for peer+token
   */
  async openChannel(params: {
    peerId: string;
    chain: string;
    token?: string;
    tokenNetwork?: string;
    peerAddress: string;
    initialDeposit?: string;
    settlementTimeout?: number;
  }): Promise<{ channelId: string; status: string }> {
    if (!this._btpServerStarted) {
      throw new ConnectorNotStartedError(
        'Connector is not started. Call start() before openChannel().'
      );
    }

    if (!this._channelManager) {
      throw new Error('Settlement infrastructure not enabled');
    }

    // Validate peer exists
    const existingPeers = this._btpClientManager.getPeerIds();
    if (!existingPeers.includes(params.peerId)) {
      throw new Error(`Peer '${params.peerId}' must be registered before opening channels`);
    }

    const tokenId = params.token ?? 'AGENT';

    // Resolve peer address: explicit param, then settlementPeers fallback
    const peerAddress = params.peerAddress || this._settlementPeers.get(params.peerId)?.evmAddress;
    if (!peerAddress) {
      throw new Error('Peer EVM address must be provided in params or peer registration');
    }

    // Check for existing active channel
    const existing = this._channelManager.getChannelForPeer(params.peerId, tokenId);
    if (existing && existing.status !== 'closed') {
      throw new Error(
        `Channel already exists for peer ${params.peerId} with token ${tokenId} on chain ${params.chain}`
      );
    }

    const channelId = await this._channelManager.ensureChannelExists(params.peerId, tokenId, {
      initialDeposit: BigInt(params.initialDeposit ?? '0'),
      settlementTimeout: params.settlementTimeout,
      chain: params.chain,
      peerAddress,
    });

    const metadata = this._channelManager.getChannelById(channelId);
    const status = metadata ? normalizeChannelStatus(metadata.status, this._logger) : 'opening';

    this._logger.info(
      { event: 'channel_opened', peerId: params.peerId, chain: params.chain, channelId },
      'Channel opened via direct API'
    );

    return { channelId, status };
  }

  /**
   * Get the state of a payment channel by ID.
   * Returns metadata-based state (no on-chain query) — sufficient for embedded mode polling.
   *
   * @param channelId - The channel identifier
   * @returns Object with channelId, normalized status, and chain
   * @throws ConnectorNotStartedError if connector has not been started
   * @throws Error('Settlement infrastructure not enabled') if channelManager is null
   * @throws Error('Channel not found: ...') if channel does not exist
   */
  async getChannelState(channelId: string): Promise<{
    channelId: string;
    status: 'opening' | 'open' | 'closed' | 'settled';
    chain: string;
  }> {
    if (!this._btpServerStarted) {
      throw new ConnectorNotStartedError(
        'Connector is not started. Call start() before getChannelState().'
      );
    }

    if (!this._channelManager) {
      throw new Error('Settlement infrastructure not enabled');
    }

    const metadata = this._channelManager.getChannelById(channelId);
    if (!metadata) {
      throw new Error(`Channel not found: ${channelId}`);
    }

    return {
      channelId: metadata.channelId,
      status: normalizeChannelStatus(metadata.status, this._logger) as
        | 'opening'
        | 'open'
        | 'closed'
        | 'settled',
      chain: metadata.chain,
    };
  }

  /**
   * Apply settlement configuration for a peer.
   * Converts AdminSettlementConfig to SettlementPeerConfig and stores/merges.
   * @private
   */
  private _applySettlementConfig(
    peerId: string,
    s: AdminSettlementConfig,
    routes: Array<{ prefix: string; priority?: number }> | undefined,
    isUpdate: boolean
  ): void {
    const ilpAddress = routes && routes.length > 0 ? routes[0]!.prefix : '';

    // Build settlementTokens
    const settlementTokens: string[] = [];
    if (s.tokenAddress) {
      settlementTokens.push(s.tokenAddress);
    } else {
      if (s.evmAddress) settlementTokens.push('EVM');
    }

    const newConfig: SettlementPeerConfig = {
      peerId,
      address: ilpAddress,
      settlementPreference: s.preference,
      settlementTokens,
      evmAddress: s.evmAddress,
      tokenAddress: s.tokenAddress,
      tokenNetworkAddress: s.tokenNetworkAddress,
      chainId: s.chainId,
      channelId: s.channelId,
      initialDeposit: s.initialDeposit,
    };

    if (isUpdate) {
      const existingConfig = this._settlementPeers.get(peerId);
      if (existingConfig) {
        const mergedConfig: SettlementPeerConfig = { ...existingConfig };
        for (const [key, value] of Object.entries(newConfig)) {
          if (value !== undefined) {
            (mergedConfig as unknown as Record<string, unknown>)[key] = value;
          }
        }
        this._settlementPeers.set(peerId, mergedConfig);
      } else {
        this._settlementPeers.set(peerId, newConfig);
      }
      this._logger.info(
        { event: 'settlement_config_merged', peerId, preference: s.preference },
        `Merged settlement config for peer: ${peerId}`
      );
    } else {
      this._settlementPeers.set(peerId, newConfig);
      this._logger.info(
        { event: 'settlement_config_added', peerId, preference: s.preference },
        `Added settlement config for peer: ${peerId}`
      );
    }
  }

  /**
   * Get routing table entries
   * @returns Array of current routing table entries
   */
  getRoutingTable(): RoutingTableEntry[] {
    return this._routingTable.getAllRoutes();
  }
}
