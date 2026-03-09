/**
 * ExplorerServer - Embedded HTTP/WebSocket server for Explorer UI
 *
 * Provides:
 * - Static file serving for Explorer UI bundle
 * - WebSocket endpoint for real-time event streaming
 * - REST API for historical event queries
 * - Health endpoint for node status
 *
 * @packageDocumentation
 */

import type { Express, Request, Response } from 'express';
import { createServer, Server as HTTPServer } from 'http';
import { WebSocketServer } from 'ws';
import path from 'path';
import { EventStore, EventQueryFilter } from './event-store';
import { TelemetryEmitter } from '../telemetry/telemetry-emitter';
import { Logger } from '../utils/logger';
import { EventBroadcaster } from './event-broadcaster';
import { requireOptional } from '../utils/optional-require';

/**
 * Configuration for ExplorerServer.
 */
/**
 * Peer information returned by the peers API endpoint.
 */
export interface PeerInfo {
  peerId: string;
  ilpAddress: string;
  evmAddress?: string;
  btpUrl?: string;
  connected: boolean;
  petname?: string;
  pubkey?: string;
}

/**
 * Routing table entry returned by the routes API endpoint.
 */
export interface RouteInfo {
  prefix: string;
  nextHop: string;
  priority?: number;
}

export interface ExplorerServerConfig {
  /** Server port (default: 3001) */
  port: number;
  /** Path to static UI files (default: './dist/explorer-ui') */
  staticPath?: string;
  /** CORS allowed origins (default: localhost patterns) */
  corsOrigins?: string[];
  /** Connector node ID for health status */
  nodeId: string;
  /** Optional callback to fetch on-chain wallet balances */
  balancesFetcher?: () => Promise<unknown>;
  /** Optional callback to fetch peer information */
  peersFetcher?: () => Promise<PeerInfo[]>;
  /** Optional callback to fetch routing table entries */
  routesFetcher?: () => Promise<RouteInfo[]>;
}

// Default configuration values
const DEFAULT_PORT = 3001;
const DEFAULT_STATIC_PATH = './dist/explorer-ui';
const DEFAULT_CORS_ORIGINS = [
  'http://localhost:3000',
  'http://localhost:3001',
  'http://localhost:5173', // Vite dev server
];
const SHUTDOWN_TIMEOUT_MS = 10000;

/**
 * ExplorerServer provides an embedded HTTP/WebSocket server for the Explorer UI.
 *
 * Features:
 * - Static file serving for SPA bundle
 * - WebSocket endpoint for live event streaming
 * - REST API for historical event queries
 * - Health endpoint with node status
 * - CORS configuration for local development
 * - Graceful shutdown with timeout
 */
export class ExplorerServer {
  private readonly _config: Required<
    Omit<ExplorerServerConfig, 'corsOrigins' | 'balancesFetcher' | 'peersFetcher' | 'routesFetcher'>
  > & {
    corsOrigins: (string | RegExp)[];
  };
  private readonly _eventStore: EventStore;
  private readonly _logger: Logger;
  private _app!: Express;
  private _express!: typeof import('express');
  private _server!: HTTPServer;
  private _wss!: WebSocketServer;
  private _broadcaster!: EventBroadcaster;
  private readonly _balancesFetcher?: () => Promise<unknown>;
  private readonly _peersFetcher?: () => Promise<PeerInfo[]>;
  private readonly _routesFetcher?: () => Promise<RouteInfo[]>;
  private readonly _telemetryEmitter: TelemetryEmitter | null;
  private _unsubscribe: (() => void) | null = null;
  private _port: number = 0;
  private _started: boolean = false;

  /**
   * Create an ExplorerServer instance.
   *
   * @param config - ExplorerServer configuration
   * @param eventStore - EventStore instance for historical queries
   * @param telemetryEmitter - TelemetryEmitter for live event subscription (optional for standalone mode)
   * @param logger - Pino logger instance
   */
  constructor(
    config: ExplorerServerConfig,
    eventStore: EventStore,
    telemetryEmitter: TelemetryEmitter | null,
    logger: Logger
  ) {
    this._config = {
      port: config.port ?? DEFAULT_PORT,
      staticPath: config.staticPath ?? DEFAULT_STATIC_PATH,
      corsOrigins: config.corsOrigins ?? [
        ...DEFAULT_CORS_ORIGINS,
        /^http:\/\/localhost:\d+$/, // Any localhost port
      ],
      nodeId: config.nodeId,
    };
    this._eventStore = eventStore;
    this._balancesFetcher = config.balancesFetcher;
    this._peersFetcher = config.peersFetcher;
    this._routesFetcher = config.routesFetcher;
    this._telemetryEmitter = telemetryEmitter;
    this._logger = logger.child({ component: 'ExplorerServer' });
  }

  /**
   * Initialize Express app and HTTP/WebSocket servers (called from start())
   */
  private async _initApp(): Promise<void> {
    const { default: express } = await requireOptional<{ default: typeof import('express') }>(
      'express',
      'HTTP admin/health APIs'
    );

    // Initialize Express app
    this._express = express;
    this._app = express();
    this._setupCors();
    this._app.use(express.json());

    // Create HTTP server
    this._server = createServer(this._app);

    // Create WebSocket server attached to HTTP server
    this._wss = new WebSocketServer({
      server: this._server,
      path: '/ws',
    });

    // Create EventBroadcaster
    this._broadcaster = new EventBroadcaster(this._wss, this._logger);

    // Subscribe to TelemetryEmitter for live event broadcasting (only if available)
    if (this._telemetryEmitter) {
      this._unsubscribe = this._telemetryEmitter.onEvent((event) => {
        this._broadcaster.broadcast(event);
      });
    }

    // Setup routes
    this._setupRoutes();
  }

  /**
   * Configure CORS middleware.
   * @private
   */
  private _setupCors(): void {
    this._app.use((req: Request, res: Response, next) => {
      const origin = req.headers.origin;

      if (origin) {
        // Check if origin matches any allowed pattern
        const isAllowed = this._config.corsOrigins.some((allowed) => {
          if (typeof allowed === 'string') {
            return origin === allowed;
          }
          return allowed.test(origin);
        });

        if (isAllowed) {
          res.setHeader('Access-Control-Allow-Origin', origin);
          res.setHeader('Access-Control-Allow-Methods', 'GET, OPTIONS');
          res.setHeader('Access-Control-Allow-Headers', 'Content-Type');
        }
      }

      // Handle preflight
      if (req.method === 'OPTIONS') {
        res.status(204).end();
        return;
      }

      next();
    });
  }

  /**
   * Setup Express routes.
   * @private
   */
  private _setupRoutes(): void {
    // API routes
    this._setupApiRoutes();

    // Static file serving
    this._setupStaticRoutes();
  }

  /**
   * Setup API routes.
   * @private
   */
  private _setupApiRoutes(): void {
    const router = this._express.Router();

    // GET /api/events - Historical event queries
    router.get('/api/events', async (req: Request, res: Response) => {
      try {
        // Parse and validate query parameters
        const limit = req.query.limit ? Number(req.query.limit) : 50;
        const offset = req.query.offset ? Number(req.query.offset) : 0;

        // Validate limit
        if (isNaN(limit) || limit < 1 || limit > 100) {
          res.status(400).json({ error: 'limit must be between 1 and 100' });
          return;
        }

        // Validate offset
        if (isNaN(offset) || offset < 0) {
          res.status(400).json({ error: 'offset must be non-negative' });
          return;
        }

        // Validate timestamps
        const since = req.query.since ? Number(req.query.since) : undefined;
        const until = req.query.until ? Number(req.query.until) : undefined;

        if (since !== undefined && isNaN(since)) {
          res.status(400).json({ error: 'since must be a valid timestamp' });
          return;
        }

        if (until !== undefined && isNaN(until)) {
          res.status(400).json({ error: 'until must be a valid timestamp' });
          return;
        }

        const filter: EventQueryFilter = {
          eventTypes: req.query.types ? String(req.query.types).split(',') : undefined,
          since,
          until,
          peerId: req.query.peerId ? String(req.query.peerId) : undefined,
          packetId: req.query.packetId ? String(req.query.packetId) : undefined,
          direction: req.query.direction as 'sent' | 'received' | 'internal' | undefined,
          limit,
          offset,
        };

        const events = await this._eventStore.queryEvents(filter);
        const total = await this._eventStore.countEvents(filter);

        res.json({ events, total, limit, offset });
      } catch (error) {
        this._logger.error({ error }, 'Failed to query events');
        res.status(500).json({ error: 'Failed to query events' });
      }
    });

    // GET /api/health - Node health status
    router.get('/api/health', async (_req: Request, res: Response) => {
      try {
        const eventCount = await this._eventStore.getEventCount();
        const dbSize = await this._eventStore.getDatabaseSize();
        const wsClients = this._broadcaster.getClientCount();

        res.json({
          status: 'healthy',
          nodeId: this._config.nodeId,
          uptime: process.uptime(),
          explorer: {
            eventCount,
            databaseSizeBytes: dbSize,
            wsConnections: wsClients,
          },
          timestamp: new Date().toISOString(),
        });
      } catch (error) {
        this._logger.error({ error }, 'Health check failed');
        res.json({
          status: 'degraded',
          nodeId: this._config.nodeId,
          uptime: process.uptime(),
          explorer: {
            eventCount: 0,
            databaseSizeBytes: 0,
            wsConnections: this._broadcaster.getClientCount(),
          },
          error: error instanceof Error ? error.message : 'Unknown error',
          timestamp: new Date().toISOString(),
        });
      }
    });

    // GET /api/accounts/events - Account/channel event replay for hydration
    router.get('/api/accounts/events', async (req: Request, res: Response) => {
      try {
        const limit = req.query.limit ? Number(req.query.limit) : 1000;

        if (isNaN(limit) || limit < 1 || limit > 5000) {
          res.status(400).json({ error: 'limit must be between 1 and 5000' });
          return;
        }

        const types = req.query.types ? String(req.query.types).split(',') : undefined;

        const filter: EventQueryFilter = {
          eventTypes: types,
          limit,
          offset: 0,
        };

        const events = await this._eventStore.queryEvents(filter, 'ASC');
        const total = await this._eventStore.countEvents(filter);

        res.json({ events, total });
      } catch (error) {
        this._logger.error({ error }, 'Failed to query account events');
        res.status(500).json({ error: 'Failed to query account events' });
      }
    });

    // GET /api/balances - On-chain wallet balances
    router.get('/api/balances', async (_req: Request, res: Response) => {
      if (!this._balancesFetcher) {
        res.status(404).json({ error: 'Balances not available' });
        return;
      }

      try {
        const balances = await this._balancesFetcher();
        res.json(balances);
      } catch (error) {
        this._logger.error({ error }, 'Failed to fetch balances');
        res.status(500).json({ error: 'Failed to fetch balances' });
      }
    });

    // GET /api/peers - Connected peer information
    router.get('/api/peers', async (_req: Request, res: Response) => {
      if (!this._peersFetcher) {
        res.status(404).json({ error: 'Peers not available' });
        return;
      }

      try {
        const peers = await this._peersFetcher();
        res.json({ peers });
      } catch (error) {
        this._logger.error({ error }, 'Failed to fetch peers');
        res.json({ peers: [] });
      }
    });

    // GET /api/routes - Routing table entries
    router.get('/api/routes', async (_req: Request, res: Response) => {
      if (!this._routesFetcher) {
        res.status(404).json({ error: 'Routes not available' });
        return;
      }

      try {
        const routes = await this._routesFetcher();
        res.json({ routes });
      } catch (error) {
        this._logger.error({ error }, 'Failed to fetch routes');
        res.json({ routes: [] });
      }
    });

    this._app.use(router);
  }

  /**
   * Setup static file serving routes.
   * @private
   */
  private _setupStaticRoutes(): void {
    const staticPath = path.resolve(this._config.staticPath);

    // Serve static files
    this._app.use(this._express.static(staticPath));

    // SPA fallback - serve index.html for non-API routes without file extensions
    this._app.get('*', (req: Request, res: Response) => {
      // Skip API routes and WebSocket path
      if (req.path.startsWith('/api') || req.path === '/ws') {
        res.status(404).json({ error: 'Not found' });
        return;
      }

      // Check if request has a file extension (likely a missing static file)
      const hasExtension = /\.[^/]+$/.test(req.path);
      if (hasExtension) {
        // Static file requests that weren't served should 404
        res.status(404).json({ error: 'Not found' });
        return;
      }

      // SPA routing: serve index.html for paths without extensions
      const indexPath = path.join(staticPath, 'index.html');
      res.sendFile(indexPath, (err) => {
        if (err) {
          // Static path doesn't exist or index.html not found
          res.status(404).json({ error: 'Not found' });
        }
      });
    });
  }

  /**
   * Start the ExplorerServer.
   *
   * @returns Promise that resolves when server is listening
   * @throws Error if port is already in use
   */
  async start(): Promise<void> {
    await this._initApp();
    return new Promise((resolve, reject) => {
      this._server.on('error', (error: NodeJS.ErrnoException) => {
        if (error.code === 'EADDRINUSE') {
          const errorMessage = `Explorer port ${this._config.port} is already in use`;
          this._logger.error({
            event: 'explorer_server_start_failed',
            port: this._config.port,
            error: errorMessage,
          });
          reject(new Error(errorMessage));
        } else {
          this._logger.error({ event: 'explorer_server_error', error: error.message });
          reject(error);
        }
      });

      this._server.listen(this._config.port, () => {
        const address = this._server.address();
        this._port = typeof address === 'object' && address ? address.port : this._config.port;
        this._started = true;
        this._logger.info({
          event: 'explorer_server_started',
          port: this._port,
          staticPath: this._config.staticPath,
        });
        resolve();
      });
    });
  }

  /**
   * Stop the ExplorerServer gracefully.
   *
   * Cleanup sequence:
   * 1. Unsubscribe from TelemetryEmitter
   * 2. Close all WebSocket connections
   * 3. Close WebSocket server
   * 4. Close HTTP server
   *
   * @returns Promise that resolves when server is stopped
   */
  async stop(): Promise<void> {
    if (!this._started) {
      return;
    }

    this._logger.info('Explorer server shutting down...');

    // 1. Unsubscribe from TelemetryEmitter
    if (this._unsubscribe) {
      this._unsubscribe();
      this._unsubscribe = null;
    }

    // 2. Close all WebSocket connections with close code 1001 (Going Away)
    this._broadcaster.closeAll();

    // Create timeout for forceful shutdown
    const shutdownPromise = new Promise<void>((resolve) => {
      const shutdownTimeout = setTimeout(() => {
        this._logger.warn('Forceful shutdown after timeout');
        resolve();
      }, SHUTDOWN_TIMEOUT_MS);

      // 3. Close WebSocket server
      this._wss.close(() => {
        // 4. Close HTTP server
        this._server.close(() => {
          clearTimeout(shutdownTimeout);
          this._started = false;
          this._logger.info('Explorer server stopped');
          resolve();
        });
      });
    });

    return shutdownPromise;
  }

  /**
   * Get the port the server is listening on.
   * Useful for tests using port 0 (random available port).
   *
   * @returns The port number
   */
  getPort(): number {
    return this._port;
  }

  /**
   * Get the EventBroadcaster instance.
   * Useful for testing and monitoring.
   *
   * @returns The EventBroadcaster instance
   */
  getBroadcaster(): EventBroadcaster {
    return this._broadcaster;
  }
}
