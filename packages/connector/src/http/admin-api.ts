/**
 * Admin API - HTTP endpoints for dynamic peer and route management
 * @packageDocumentation
 * @remarks
 * Provides REST API for runtime configuration of the connector:
 * - Peer management (add/remove BTP connections)
 * - Route management (add/remove routing table entries)
 *
 * **Security:**
 * - Designed for internal Docker Compose network access only
 * - Optional API key authentication
 * - Should NOT be exposed to public internet
 *
 * @example
 * ```typescript
 * const adminRouter = createAdminRouter({
 *   routingTable,
 *   btpClientManager,
 *   logger,
 *   apiKey: 'optional-secret-key'
 * });
 * app.use('/admin', adminRouter);
 * ```
 */

import { timingSafeEqual } from 'node:crypto';
import type { Router, Request, Response, NextFunction } from 'express';
import { Netmask } from 'netmask';
import { Logger } from '../utils/logger';
import { requireOptional } from '../utils/optional-require';
import { RoutingTable } from '../routing/routing-table';
import { BTPClientManager } from '../btp/btp-client-manager';
import { Peer } from '../btp/btp-client';
import { ILPAddress, isValidILPAddress } from '@toon-protocol/shared';
import {
  AdminSettlementConfig,
  PeerConfig as SettlementPeerConfig,
  isValidEvmAddress,
  isValidNonNegativeIntegerString,
  normalizeChannelStatus,
} from '../settlement/types';
import type { AdminChannelStatus } from '../settlement/types';
import type { ChannelManager } from '../settlement/channel-manager';
import type { PaymentChannelSDK } from '../settlement/payment-channel-sdk';
import type { AccountManager } from '../settlement/account-manager';
import type { SettlementMonitor } from '../settlement/settlement-monitor';
import type { ClaimReceiver } from '../settlement/claim-receiver';
import type { BlockchainType } from '../btp/btp-claim-types';
import { IlpSendHandler } from './ilp-send-handler';
import type { PacketSenderFn, IsReadyFn } from './ilp-send-handler';

/**
 * Admin API Configuration
 */
export interface AdminAPIConfig {
  /** Routing table instance for route management */
  routingTable: RoutingTable;

  /** BTP client manager for peer management */
  btpClientManager: BTPClientManager;

  /** Logger instance */
  logger: Logger;

  /** Optional API key for authentication (if not set, no auth required) */
  apiKey?: string;

  /** Optional IP allowlist for access control (supports CIDR notation) */
  allowedIPs?: string[];

  /** Trust X-Forwarded-For header for client IP (only enable behind trusted proxy) */
  trustProxy?: boolean;

  /** Node ID for logging context */
  nodeId: string;

  /**
   * Optional settlement peer config Map for storing runtime settlement configurations.
   * When provided, POST /admin/peers stores PeerConfig entries and GET /admin/peers
   * includes settlement info. If omitted, settlement features are silently skipped.
   */
  settlementPeers?: Map<string, SettlementPeerConfig>;

  /** Optional ChannelManager for payment channel lifecycle operations */
  channelManager?: ChannelManager;

  /** Optional PaymentChannelSDK for on-chain EVM channel state queries */
  paymentChannelSDK?: PaymentChannelSDK;

  /** Optional AccountManager for peer balance queries (TigerBeetle) */
  accountManager?: AccountManager;

  /** Optional SettlementMonitor for settlement state queries */
  settlementMonitor?: SettlementMonitor;

  /** Optional ClaimReceiver for payment channel claim queries */
  claimReceiver?: ClaimReceiver;

  /** Optional callback for sending ILP packets via ConnectorNode.sendPacket() */
  packetSender?: PacketSenderFn;

  /** Optional callback for checking if the connector is ready to send packets */
  isReady?: IsReadyFn;

  /** Default settlement token ID resolved from on-chain ERC-20 symbol (e.g. 'M2M') */
  defaultSettlementTokenId?: string;
}

/**
 * Request body for adding a peer
 */
export interface AddPeerRequest {
  /** Unique peer identifier */
  id: string;

  /** WebSocket URL for BTP connection (e.g., ws://peer:3000) */
  url: string;

  /** Authentication token for BTP handshake */
  authToken: string;

  /** Optional routes to add for this peer */
  routes?: Array<{
    /** ILP address prefix */
    prefix: string;
    /** Route priority (higher wins, default: 0) */
    priority?: number;
  }>;

  /**
   * Optional settlement configuration for this peer.
   * When provided, a PeerConfig is created and stored for settlement routing.
   * @example
   * ```json
   * {
   *   "preference": "evm",
   *   "evmAddress": "0x742d35Cc6634C0532925a3b844Bc9e7595f2bD28",
   *   "tokenAddress": "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48",
   *   "chainId": 8453
   * }
   * ```
   */
  settlement?: AdminSettlementConfig;
}

/**
 * Request body for adding a route
 */
export interface AddRouteRequest {
  /** ILP address prefix (e.g., g.agent.alice) */
  prefix: string;

  /** Peer ID to forward packets to */
  nextHop: string;

  /** Route priority (higher wins, default: 0) */
  priority?: number;
}

/**
 * GET /admin/balances/:peerId response
 * MVP: balances array always contains a single element (one tokenId per query).
 * Array structure allows future multi-token expansion without breaking the API.
 */
export interface BalanceResponse {
  peerId: string;
  balances: Array<{
    tokenId: string;
    debitBalance: string;
    creditBalance: string;
    netBalance: string;
  }>;
}

/**
 * GET /admin/settlement/states response item
 */
export interface SettlementStateResponse {
  peerId: string;
  tokenId: string;
  state: string;
}

/**
 * Helper: Normalize IP address (convert IPv4-mapped IPv6 to IPv4)
 * @param ip - IP address (may be IPv4-mapped IPv6 like ::ffff:127.0.0.1)
 * @returns Normalized IP address
 */
function normalizeIP(ip: string): string {
  // Convert IPv4-mapped IPv6 (::ffff:192.0.2.1) to IPv4 (192.0.2.1)
  if (ip.startsWith('::ffff:')) {
    return ip.substring(7);
  }
  return ip;
}

/**
 * Helper: Extract client IP from request
 * @param req - Express request object
 * @param trustProxy - Whether to trust X-Forwarded-For header
 * @returns Client IP address (normalized)
 */
function getClientIP(req: Request, trustProxy: boolean): string {
  if (trustProxy) {
    // When behind proxy, use X-Forwarded-For (first IP is client)
    const forwardedFor = req.headers['x-forwarded-for'];
    if (forwardedFor) {
      const headerValue = typeof forwardedFor === 'string' ? forwardedFor : forwardedFor[0];
      if (headerValue) {
        const ips = headerValue.split(',');
        const firstIP = ips[0];
        if (firstIP) {
          return normalizeIP(firstIP.trim());
        }
      }
    }
  }
  // Direct connection: use socket IP
  const rawIP = req.ip || req.socket.remoteAddress || 'unknown';
  return normalizeIP(rawIP);
}

/**
 * Helper: Check if IP matches allowlist (supports CIDR notation)
 * @param ip - Client IP address
 * @param allowedIPs - Array of allowed IPs/CIDR ranges
 * @returns True if IP is allowed, false otherwise
 */
function isIPAllowed(ip: string, allowedIPs: string[]): boolean {
  for (const allowed of allowedIPs) {
    try {
      // Check if it's a CIDR range
      if (allowed.includes('/')) {
        const block = new Netmask(allowed);
        if (block.contains(ip)) {
          return true;
        }
      } else {
        // Exact IP match
        if (ip === allowed) {
          return true;
        }
      }
    } catch (err) {
      // Invalid CIDR notation — skip this entry
      continue;
    }
  }
  return false;
}

/**
 * Create IP allowlist middleware
 * @param allowedIPs - Array of allowed IP addresses/CIDR ranges
 * @param trustProxy - Whether to trust X-Forwarded-For header
 * @param logger - Logger instance
 * @returns Express middleware function
 */
function createIPAllowlistMiddleware(
  allowedIPs: string[],
  trustProxy: boolean,
  logger: Logger
): (req: Request, res: Response, next: NextFunction) => void {
  const log = logger.child({ component: 'IPAllowlist' });

  return (req: Request, res: Response, next: NextFunction) => {
    const clientIP = getClientIP(req, trustProxy);

    if (!isIPAllowed(clientIP, allowedIPs)) {
      log.warn(
        {
          event: 'admin_api_ip_blocked',
          ip: clientIP,
          path: req.path,
          allowedIPs,
          trustProxy,
        },
        'Admin API request blocked by IP allowlist'
      );
      res.status(403).json({
        error: 'Forbidden',
        message: 'IP address not allowed',
      });
      return;
    }

    // IP is allowed — continue to next middleware
    next();
  };
}

/**
 * Create Admin API Express router
 *
 * @param config - Admin API configuration
 * @returns Express router with admin endpoints
 *
 * @remarks
 * Endpoints:
 * - GET /admin/peers - List all peers with connection status
 * - POST /admin/peers - Add a new peer (and optionally routes)
 * - DELETE /admin/peers/:peerId - Remove a peer (and optionally its routes)
 * - GET /admin/routes - List all routes
 * - POST /admin/routes - Add a new route
 * - DELETE /admin/routes/:prefix - Remove a route
 */
export async function createAdminRouter(config: AdminAPIConfig): Promise<Router> {
  const { default: express } = await requireOptional<{ default: typeof import('express') }>(
    'express',
    'HTTP admin/health APIs'
  );
  const router = express.Router();
  const {
    routingTable,
    btpClientManager,
    logger,
    apiKey,
    allowedIPs,
    trustProxy = false,
    nodeId,
    settlementPeers,
    channelManager,
    paymentChannelSDK,
    accountManager,
    settlementMonitor,
    claimReceiver,
    packetSender,
    isReady,
    defaultSettlementTokenId,
  } = config;
  const log = logger.child({ component: 'AdminAPI' });

  // JSON body parser
  router.use(express.json());

  // Optional IP allowlist middleware (checked BEFORE API key for fast rejection)
  if (allowedIPs && allowedIPs.length > 0) {
    router.use(createIPAllowlistMiddleware(allowedIPs, trustProxy, logger));
  }

  // Optional API key authentication middleware
  if (apiKey) {
    const apiKeyBuffer = Buffer.from(apiKey);
    router.use((req: Request, res: Response, next: NextFunction) => {
      // Only accept API key via X-Api-Key header — reject query param to avoid
      // keys leaking into access logs, proxy logs, and browser history.
      if (req.query.apiKey) {
        log.warn(
          {
            event: 'admin_api_key_in_query',
            ip: req.ip,
            path: req.path,
          },
          'API key supplied via query parameter (rejected — use X-Api-Key header)'
        );
        res.status(401).json({
          error: 'Unauthorized',
          message: 'API key must be provided via X-Api-Key header, not query parameter',
        });
        return;
      }

      const providedKey = req.headers['x-api-key'];

      // Timing-safe comparison: convert to equal-length buffers to avoid
      // leaking key length via early-exit on length mismatch.
      const providedBuffer = Buffer.from(typeof providedKey === 'string' ? providedKey : '');
      const isLengthMatch = providedBuffer.length === apiKeyBuffer.length;
      // Compare against actual key when lengths match, otherwise compare against
      // a dummy of the same length as the provided key to burn constant time.
      const comparand = isLengthMatch ? apiKeyBuffer : providedBuffer;
      const isMatch = timingSafeEqual(providedBuffer, comparand) && isLengthMatch;

      if (!isMatch) {
        log.warn(
          {
            event: 'admin_api_auth_failed',
            ip: req.ip,
            path: req.path,
          },
          'Admin API authentication failed'
        );
        res.status(401).json({
          error: 'Unauthorized',
          message: 'Invalid or missing API key',
        });
        return;
      }

      next();
    });
  }

  // Request logging middleware
  router.use((req: Request, _res: Response, next: NextFunction) => {
    log.info(
      {
        event: 'admin_api_request',
        method: req.method,
        path: req.path,
        ip: req.ip,
      },
      `Admin API: ${req.method} ${req.path}`
    );
    next();
  });

  /**
   * GET /admin/peers
   * List all peers with their connection status
   */
  router.get('/peers', (_req: Request, res: Response) => {
    try {
      const peerIds = btpClientManager.getPeerIds();
      const peerStatus = btpClientManager.getPeerStatus();
      const routes = routingTable.getAllRoutes();

      // Build peer response with ILP addresses from routes
      const peers = peerIds.map((peerId) => {
        // Find routes that use this peer as nextHop
        const peerRoutes = routes.filter((r) => r.nextHop === peerId);
        const ilpAddresses = peerRoutes.map((r) => r.prefix);

        const peerResponse: Record<string, unknown> = {
          id: peerId,
          connected: peerStatus.get(peerId) ?? false,
          ilpAddresses,
          routeCount: peerRoutes.length,
        };

        // Include settlement info if available
        if (settlementPeers) {
          const peerConfig = settlementPeers.get(peerId);
          if (peerConfig) {
            peerResponse.settlement = {
              preference: peerConfig.settlementPreference,
              evmAddress: peerConfig.evmAddress,
              tokenAddress: peerConfig.tokenAddress,
              tokenNetworkAddress: peerConfig.tokenNetworkAddress,
              chainId: peerConfig.chainId,
              channelId: peerConfig.channelId,
              initialDeposit: peerConfig.initialDeposit,
            };
          }
        }

        return peerResponse;
      });

      res.json({
        nodeId,
        peerCount: peers.length,
        connectedCount: peers.filter((p) => p.connected).length,
        peers,
      });
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : String(error);
      log.error({ event: 'admin_api_error', error: errorMessage }, 'Failed to list peers');
      res.status(500).json({ error: 'Internal server error', message: errorMessage });
    }
  });

  /**
   * POST /admin/peers
   * Add a new peer with optional routes
   */
  router.post('/peers', async (req: Request, res: Response) => {
    try {
      const body = req.body as AddPeerRequest;

      // Validate required fields
      if (!body.id || typeof body.id !== 'string') {
        res.status(400).json({ error: 'Bad request', message: 'Missing or invalid peer id' });
        return;
      }
      if (!body.url || typeof body.url !== 'string') {
        res.status(400).json({ error: 'Bad request', message: 'Missing or invalid peer url' });
        return;
      }
      if (
        body.authToken === undefined ||
        body.authToken === null ||
        typeof body.authToken !== 'string'
      ) {
        res.status(400).json({
          error: 'Bad request',
          message: 'authToken must be a string (can be empty for no auth)',
        });
        return;
      }

      // Validate URL format
      if (!body.url.startsWith('ws://') && !body.url.startsWith('wss://')) {
        res.status(400).json({
          error: 'Bad request',
          message: 'URL must start with ws:// or wss://',
        });
        return;
      }

      // Check if peer already exists (idempotent re-registration)
      const existingPeers = btpClientManager.getPeerIds();
      const isUpdate = existingPeers.includes(body.id);

      // Validate routes if provided
      if (body.routes) {
        for (const route of body.routes) {
          if (!route.prefix || typeof route.prefix !== 'string') {
            res.status(400).json({
              error: 'Bad request',
              message: 'Invalid route: missing prefix',
            });
            return;
          }
          if (!isValidILPAddress(route.prefix)) {
            res.status(400).json({
              error: 'Bad request',
              message: `Invalid ILP address prefix: ${route.prefix}`,
            });
            return;
          }
        }
      }

      // Validate settlement config if provided
      if (body.settlement) {
        const settlementError = validateSettlementConfig(body.settlement);
        if (settlementError) {
          res.status(400).json({ error: 'Bad request', message: settlementError });
          return;
        }
      }

      // Only add BTP peer on initial registration (BTP connection doesn't change on re-registration)
      if (!isUpdate) {
        const peer: Peer = {
          id: body.id,
          url: body.url,
          authToken: body.authToken,
          connected: false,
          lastSeen: new Date(),
        };

        await btpClientManager.addPeer(peer);

        log.info(
          { event: 'admin_peer_added', peerId: body.id, url: body.url },
          `Added peer: ${body.id}`
        );
      } else {
        log.info(
          { event: 'admin_peer_reregistered', peerId: body.id },
          `Re-registering peer: ${body.id}`
        );
      }

      // Add routes if provided (addRoute replaces existing same-prefix routes, no duplicates)
      const addedRoutes: string[] = [];
      if (body.routes) {
        for (const route of body.routes) {
          routingTable.addRoute(route.prefix as ILPAddress, body.id, route.priority ?? 0);
          addedRoutes.push(route.prefix);
          log.info(
            { event: 'admin_route_added', prefix: route.prefix, nextHop: body.id },
            `Added route: ${route.prefix} -> ${body.id}`
          );
        }
      }

      // Create/merge PeerConfig if settlement provided and settlementPeers available
      if (body.settlement && settlementPeers) {
        const s = body.settlement;
        const ilpAddress = body.routes && body.routes.length > 0 ? body.routes[0]!.prefix : '';

        // Build settlementTokens
        const settlementTokens: string[] = [];
        if (s.tokenAddress) {
          settlementTokens.push(s.tokenAddress);
        } else {
          if (s.evmAddress) settlementTokens.push('EVM');
        }

        const newConfig: SettlementPeerConfig = {
          peerId: body.id,
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
          // Merge: spread existing config, overwrite with new non-undefined fields
          const existingConfig = settlementPeers.get(body.id);
          if (existingConfig) {
            const mergedConfig: SettlementPeerConfig = { ...existingConfig };
            for (const [key, value] of Object.entries(newConfig)) {
              if (value !== undefined) {
                (mergedConfig as unknown as Record<string, unknown>)[key] = value;
              }
            }
            settlementPeers.set(body.id, mergedConfig);
          } else {
            settlementPeers.set(body.id, newConfig);
          }
          log.info(
            {
              event: 'admin_settlement_config_merged',
              peerId: body.id,
              preference: s.preference,
            },
            `Merged settlement config for peer: ${body.id}`
          );
        } else {
          settlementPeers.set(body.id, newConfig);
          log.info(
            {
              event: 'admin_settlement_config_added',
              peerId: body.id,
              preference: s.preference,
            },
            `Added settlement config for peer: ${body.id}`
          );
        }
      }

      if (isUpdate) {
        // Return 200 for re-registration
        const connected = btpClientManager.isConnected(body.id);
        res.status(200).json({
          success: true,
          peer: {
            id: body.id,
            url: body.url,
            connected,
          },
          routes: addedRoutes,
          updated: true,
          message: `Peer '${body.id}' updated`,
        });
      } else {
        // Check connection status after a brief delay for new peers
        await new Promise((resolve) => setTimeout(resolve, 1000));
        const connected = btpClientManager.isConnected(body.id);

        res.status(201).json({
          success: true,
          peer: {
            id: body.id,
            url: body.url,
            connected,
          },
          routes: addedRoutes,
          created: true,
          message: connected
            ? `Peer '${body.id}' added and connected`
            : `Peer '${body.id}' added (connection pending)`,
        });
      }
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : String(error);
      log.error({ event: 'admin_api_error', error: errorMessage }, 'Failed to add peer');
      res.status(500).json({ error: 'Internal server error', message: errorMessage });
    }
  });

  /**
   * DELETE /admin/peers/:peerId
   * Remove a peer and optionally its routes
   */
  router.delete('/peers/:peerId', async (req: Request, res: Response) => {
    try {
      const peerId = req.params.peerId;
      if (!peerId) {
        res.status(400).json({ error: 'Bad request', message: 'Missing peerId parameter' });
        return;
      }
      const removeRoutes = req.query.removeRoutes !== 'false'; // Default: true

      // Check if peer exists
      const existingPeers = btpClientManager.getPeerIds();
      if (!existingPeers.includes(peerId)) {
        res.status(404).json({
          error: 'Not found',
          message: `Peer '${peerId}' not found`,
        });
        return;
      }

      // Remove peer
      await btpClientManager.removePeer(peerId);
      log.info({ event: 'admin_peer_removed', peerId }, `Removed peer: ${peerId}`);

      // Remove settlement config if exists
      if (settlementPeers && settlementPeers.delete(peerId)) {
        log.info(
          { event: 'admin_settlement_config_removed', peerId },
          `Removed settlement config for peer: ${peerId}`
        );
      }

      // Remove routes if requested
      const removedRoutes: string[] = [];
      if (removeRoutes) {
        const routes = routingTable.getAllRoutes();
        for (const route of routes) {
          if (route.nextHop === peerId) {
            routingTable.removeRoute(route.prefix);
            removedRoutes.push(route.prefix);
            log.info(
              { event: 'admin_route_removed', prefix: route.prefix },
              `Removed route: ${route.prefix}`
            );
          }
        }
      }

      res.json({
        success: true,
        peerId,
        removedRoutes,
        message: `Peer '${peerId}' removed${removedRoutes.length > 0 ? ` with ${removedRoutes.length} routes` : ''}`,
      });
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : String(error);
      log.error({ event: 'admin_api_error', error: errorMessage }, 'Failed to remove peer');
      res.status(500).json({ error: 'Internal server error', message: errorMessage });
    }
  });

  /**
   * PUT /admin/peers/:peerId
   * Update an existing peer's settlement config and/or routes
   */
  router.put('/peers/:peerId', (req: Request, res: Response) => {
    try {
      const peerId = req.params.peerId;
      if (!peerId) {
        res.status(400).json({ error: 'Bad request', message: 'Missing peerId parameter' });
        return;
      }

      // Validate peerId exists
      const existingPeers = btpClientManager.getPeerIds();
      if (!existingPeers.includes(peerId)) {
        res.status(404).json({
          error: 'Not found',
          message: 'Peer not found',
        });
        return;
      }

      const body = req.body as {
        settlement?: AdminSettlementConfig;
        routes?: Array<{ prefix: string; priority?: number }>;
      };

      // Validate settlement config if provided
      if (body.settlement) {
        const settlementError = validateSettlementConfig(body.settlement);
        if (settlementError) {
          res.status(400).json({ error: 'Bad request', message: settlementError });
          return;
        }
      }

      // Validate routes if provided
      if (body.routes) {
        for (const route of body.routes) {
          if (!route.prefix || typeof route.prefix !== 'string') {
            res.status(400).json({
              error: 'Bad request',
              message: 'Invalid route: missing prefix',
            });
            return;
          }
          if (!isValidILPAddress(route.prefix)) {
            res.status(400).json({
              error: 'Bad request',
              message: `Invalid ILP address prefix: ${route.prefix}`,
            });
            return;
          }
        }
      }

      // Update settlement config if provided
      if (body.settlement && settlementPeers) {
        const s = body.settlement;
        const existingConfig = settlementPeers.get(peerId);

        const settlementTokens: string[] = [];
        if (s.tokenAddress) {
          settlementTokens.push(s.tokenAddress);
        } else {
          if (s.evmAddress) settlementTokens.push('EVM');
        }

        const newConfig: SettlementPeerConfig = {
          peerId,
          address: existingConfig?.address ?? '',
          settlementPreference: s.preference,
          settlementTokens,
          evmAddress: s.evmAddress,
          tokenAddress: s.tokenAddress,
          tokenNetworkAddress: s.tokenNetworkAddress,
          chainId: s.chainId,
          channelId: s.channelId,
          initialDeposit: s.initialDeposit,
        };

        if (existingConfig) {
          const mergedConfig: SettlementPeerConfig = { ...existingConfig };
          for (const [key, value] of Object.entries(newConfig)) {
            if (value !== undefined) {
              (mergedConfig as unknown as Record<string, unknown>)[key] = value;
            }
          }
          settlementPeers.set(peerId, mergedConfig);
        } else {
          settlementPeers.set(peerId, newConfig);
        }

        log.info(
          { event: 'admin_peer_settlement_updated', peerId, preference: s.preference },
          `Updated settlement config for peer: ${peerId}`
        );
      }

      // Add routes if provided
      if (body.routes) {
        for (const route of body.routes) {
          routingTable.addRoute(route.prefix as ILPAddress, peerId, route.priority ?? 0);
          log.info(
            { event: 'admin_route_added', prefix: route.prefix, nextHop: peerId },
            `Added route: ${route.prefix} -> ${peerId}`
          );
        }
      }

      res.status(200).json({
        success: true,
        peerId,
        updated: true,
      });
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : String(error);
      log.error({ event: 'admin_api_error', error: errorMessage }, 'Failed to update peer');
      res.status(500).json({ error: 'Internal server error', message: errorMessage });
    }
  });

  /**
   * GET /admin/routes
   * List all routes in the routing table
   */
  router.get('/routes', (_req: Request, res: Response) => {
    try {
      const routes = routingTable.getAllRoutes();

      res.json({
        nodeId,
        routeCount: routes.length,
        routes: routes.map((r) => ({
          prefix: r.prefix,
          nextHop: r.nextHop,
          priority: r.priority ?? 0,
        })),
      });
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : String(error);
      log.error({ event: 'admin_api_error', error: errorMessage }, 'Failed to list routes');
      res.status(500).json({ error: 'Internal server error', message: errorMessage });
    }
  });

  /**
   * POST /admin/routes
   * Add a new route to the routing table
   */
  router.post('/routes', (req: Request, res: Response) => {
    try {
      const body = req.body as AddRouteRequest;

      // Validate required fields
      if (!body.prefix || typeof body.prefix !== 'string') {
        res.status(400).json({ error: 'Bad request', message: 'Missing or invalid prefix' });
        return;
      }
      if (!body.nextHop || typeof body.nextHop !== 'string') {
        res.status(400).json({ error: 'Bad request', message: 'Missing or invalid nextHop' });
        return;
      }

      // Validate ILP address format
      if (!isValidILPAddress(body.prefix)) {
        res.status(400).json({
          error: 'Bad request',
          message: `Invalid ILP address prefix: ${body.prefix}`,
        });
        return;
      }

      // Check if nextHop peer exists (warning only, don't block)
      const existingPeers = btpClientManager.getPeerIds();
      const peerExists = existingPeers.includes(body.nextHop);

      // Add route
      const priority = body.priority ?? 0;
      routingTable.addRoute(body.prefix as ILPAddress, body.nextHop, priority);

      log.info(
        { event: 'admin_route_added', prefix: body.prefix, nextHop: body.nextHop, priority },
        `Added route: ${body.prefix} -> ${body.nextHop}`
      );

      res.status(201).json({
        success: true,
        route: {
          prefix: body.prefix,
          nextHop: body.nextHop,
          priority,
        },
        warning: peerExists ? undefined : `Peer '${body.nextHop}' does not exist yet`,
        message: `Route '${body.prefix}' -> '${body.nextHop}' added`,
      });
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : String(error);
      log.error({ event: 'admin_api_error', error: errorMessage }, 'Failed to add route');
      res.status(500).json({ error: 'Internal server error', message: errorMessage });
    }
  });

  /**
   * DELETE /admin/routes/:prefix
   * Remove a route from the routing table
   *
   * Note: prefix is URL-encoded (e.g., g.agent.alice becomes g.agent.alice)
   * Use encodeURIComponent for prefixes with special characters
   */
  router.delete('/routes/:prefix(*)', (req: Request, res: Response) => {
    try {
      const rawPrefix = req.params.prefix;
      if (!rawPrefix) {
        res.status(400).json({ error: 'Bad request', message: 'Missing prefix parameter' });
        return;
      }
      const prefix = decodeURIComponent(rawPrefix);

      // Check if route exists
      const routes = routingTable.getAllRoutes();
      const existingRoute = routes.find((r) => r.prefix === prefix);

      if (!existingRoute) {
        res.status(404).json({
          error: 'Not found',
          message: `Route with prefix '${prefix}' not found`,
        });
        return;
      }

      // Remove route
      routingTable.removeRoute(prefix);

      log.info({ event: 'admin_route_removed', prefix }, `Removed route: ${prefix}`);

      res.json({
        success: true,
        prefix,
        message: `Route '${prefix}' removed`,
      });
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : String(error);
      log.error({ event: 'admin_api_error', error: errorMessage }, 'Failed to remove route');
      res.status(500).json({ error: 'Internal server error', message: errorMessage });
    }
  });

  // --- Payment Channel Endpoints ---

  /**
   * POST /admin/channels
   * Open a new payment channel
   */
  router.post('/channels', async (req: Request, res: Response) => {
    try {
      if (!channelManager) {
        res.status(503).json({
          error: 'Service Unavailable',
          message: 'Settlement infrastructure not enabled',
        });
        return;
      }

      const validation = validateOpenChannelRequest(req.body as Record<string, unknown>);
      if (!validation.valid) {
        res.status(400).json({ error: 'Bad request', message: validation.error });
        return;
      }

      const body = req.body as OpenChannelRequest;

      // Validate peer exists before opening channels
      const existingPeers = btpClientManager.getPeerIds();
      if (!existingPeers.includes(body.peerId)) {
        res.status(404).json({
          error: 'Not found',
          message: `Peer '${body.peerId}' must be registered before opening channels`,
        });
        return;
      }

      const chainPrefix = body.chain.split(':')[0];

      if (chainPrefix === 'evm') {
        // Derive tokenId from request
        const tokenId = body.token ?? 'AGENT';

        // Resolve peer EVM address: explicit request field, then settlementPeers fallback
        const peerConfig = settlementPeers?.get(body.peerId);
        const peerAddress = body.peerAddress || peerConfig?.evmAddress;
        if (!peerAddress) {
          res.status(400).json({
            error: 'Bad request',
            message: 'Peer EVM address must be provided in request or peer registration',
          });
          return;
        }

        // Validate EVM address format if provided in request
        if (body.peerAddress && !/^0x[0-9a-fA-F]{40}$/.test(body.peerAddress)) {
          res.status(400).json({
            error: 'Bad request',
            message: 'Invalid EVM address format: must be 0x-prefixed 42-char hex',
          });
          return;
        }

        const addressSource = body.peerAddress ? 'request' : 'registration';
        log.info(
          { peerId: body.peerId, peerAddress, source: addressSource },
          `Resolved peer EVM address from ${addressSource}`
        );

        // Check for existing channel
        const existing = channelManager.getChannelForPeer(body.peerId, tokenId);
        if (existing && existing.status !== 'closed') {
          res.status(409).json({
            error: 'Conflict',
            message: `Channel already exists for peer ${body.peerId} with token ${tokenId} on chain ${body.chain}`,
          });
          return;
        }

        const channelId = await channelManager.ensureChannelExists(body.peerId, tokenId, {
          initialDeposit: BigInt(body.initialDeposit),
          settlementTimeout: body.settlementTimeout,
          chain: body.chain,
          peerAddress,
        });

        log.info(
          { peerId: body.peerId, chain: body.chain, channelId },
          'Channel opened via Admin API'
        );

        const metadata = channelManager.getChannelById(channelId);
        if (!metadata) {
          res.status(500).json({
            error: 'Internal error',
            message: 'Channel created but metadata unavailable',
          });
          return;
        }

        res.status(201).json({
          channelId,
          chain: body.chain,
          status: normalizeChannelStatus(metadata.status, log),
          deposit: body.initialDeposit,
        } satisfies OpenChannelResponse);
      } else {
        res.status(400).json({
          error: 'Bad request',
          message: `Unsupported blockchain: ${chainPrefix}`,
        });
      }
    } catch (error) {
      log.error(
        {
          err: error,
          peerId: (req.body as Record<string, unknown>).peerId,
          chain: (req.body as Record<string, unknown>).chain,
        },
        'Channel open failed'
      );
      res.status(500).json({ error: 'Internal error', message: 'Channel open failed' });
    }
  });

  /**
   * GET /admin/channels
   * List all channels with optional filters
   */
  router.get('/channels', async (_req: Request, res: Response) => {
    try {
      if (!channelManager) {
        res.status(503).json({
          error: 'Service Unavailable',
          message: 'Settlement infrastructure not enabled',
        });
        return;
      }

      let channels = channelManager.getAllChannels();

      // Apply optional query filters
      const filterPeerId = _req.query.peerId as string | undefined;
      const filterChain = _req.query.chain as string | undefined;
      const filterStatus = _req.query.status as string | undefined;

      if (filterPeerId) {
        channels = channels.filter((ch) => ch.peerId === filterPeerId);
      }
      if (filterChain) {
        channels = channels.filter((ch) => ch.chain === filterChain);
      }
      if (filterStatus) {
        const normalizedFilter = normalizeChannelStatus(filterStatus, log);
        channels = channels.filter(
          (ch) => normalizeChannelStatus(ch.status, log) === normalizedFilter
        );
      }

      // Map to response format
      const summaries: ChannelSummary[] = channels.map((ch) => ({
        channelId: ch.channelId,
        peerId: ch.peerId,
        chain: ch.chain,
        status: normalizeChannelStatus(ch.status, log),
        deposit: 'unknown',
        lastActivity: ch.lastActivityAt.toISOString(),
      }));

      // Try to enrich with on-chain deposit info (parallel queries)
      if (paymentChannelSDK) {
        await Promise.all(
          summaries.map(async (summary) => {
            try {
              const ch = channels.find((c) => c.channelId === summary.channelId);
              if (ch) {
                const state = await paymentChannelSDK.getChannelState(
                  ch.channelId,
                  ch.tokenAddress
                );
                summary.deposit = state.myDeposit.toString();
              }
            } catch {
              // Leave as 'unknown' if query fails
            }
          })
        );
      }

      res.json(summaries);
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : String(error);
      log.error({ event: 'admin_api_error', error: errorMessage }, 'Failed to list channels');
      res.status(500).json({ error: 'Internal server error', message: errorMessage });
    }
  });

  /**
   * GET /admin/channels/:channelId
   * Get channel details with on-chain state
   */
  router.get('/channels/:channelId', async (req: Request, res: Response) => {
    try {
      if (!channelManager) {
        res.status(503).json({
          error: 'Service Unavailable',
          message: 'Settlement infrastructure not enabled',
        });
        return;
      }

      const reqChannelId = req.params.channelId as string;
      const metadata = channelManager.getChannelById(reqChannelId);

      if (!metadata) {
        res.status(404).json({ error: 'Not found', message: 'Channel not found' });
        return;
      }

      // Query on-chain state if SDK available
      if (paymentChannelSDK && metadata.chain.startsWith('evm')) {
        const state = await paymentChannelSDK.getChannelState(
          metadata.channelId,
          metadata.tokenAddress
        );

        // Serialize BigInt values to strings
        res.json({
          channelId: state.channelId,
          participants: state.participants,
          deposit: state.myDeposit.toString(),
          theirDeposit: state.theirDeposit.toString(),
          transferred: state.myTransferred.toString(),
          theirTransferred: state.theirTransferred.toString(),
          status: normalizeChannelStatus(state.status, log),
          nonce: state.myNonce,
          theirNonce: state.theirNonce,
          settlementTimeout: state.settlementTimeout,
          openedAt: state.openedAt,
          closedAt: state.closedAt,
        } satisfies ChannelDetailResponse);
        return;
      }

      // Fallback: return metadata only (non-EVM or SDK unavailable)
      res.json({
        channelId: metadata.channelId,
        peerId: metadata.peerId,
        chain: metadata.chain,
        status: normalizeChannelStatus(metadata.status, log),
        deposit: 'unknown',
        tokenId: metadata.tokenId,
        createdAt: metadata.createdAt.toISOString(),
        lastActivity: metadata.lastActivityAt.toISOString(),
      } satisfies ChannelDetailResponse);
    } catch (error) {
      log.error({ err: error, channelId: req.params.channelId }, 'Failed to query channel state');
      res.status(500).json({ error: 'Internal error', message: 'Failed to query channel state' });
    }
  });

  /**
   * POST /admin/channels/:channelId/deposit
   * Add funds to a payment channel
   */
  router.post('/channels/:channelId/deposit', async (req: Request, res: Response) => {
    try {
      if (!channelManager) {
        res.status(503).json({
          error: 'Service Unavailable',
          message: 'Settlement infrastructure not enabled',
        });
        return;
      }

      const reqChannelId = req.params.channelId as string;
      const metadata = channelManager.getChannelById(reqChannelId);

      if (!metadata) {
        res.status(404).json({ error: 'Not found', message: 'Channel not found' });
        return;
      }

      const validation = validateDepositRequest(req.body as Record<string, unknown>);
      if (!validation.valid) {
        res.status(400).json({ error: 'Bad request', message: validation.error });
        return;
      }

      if (normalizeChannelStatus(metadata.status, log) !== 'open') {
        res.status(400).json({
          error: 'Bad request',
          message: 'Channel is not in open state',
        });
        return;
      }

      const { amount } = req.body as DepositRequest;
      const chainPrefix = metadata.chain.split(':')[0];

      if (chainPrefix === 'evm') {
        if (!paymentChannelSDK) {
          res.status(503).json({
            error: 'Service Unavailable',
            message: 'EVM settlement infrastructure not enabled',
          });
          return;
        }

        await paymentChannelSDK.deposit(reqChannelId, metadata.tokenAddress, BigInt(amount));

        const state = await paymentChannelSDK.getChannelState(reqChannelId, metadata.tokenAddress);

        metadata.lastActivityAt = new Date();

        log.info(
          { channelId: reqChannelId, chain: chainPrefix, amount },
          'Deposit completed via Admin API'
        );

        res.json({
          channelId: reqChannelId,
          newDeposit: state.myDeposit.toString(),
          status: normalizeChannelStatus(metadata.status, log),
        } satisfies DepositResponse);
      } else {
        res.status(400).json({
          error: 'Bad request',
          message: `Unsupported blockchain: ${chainPrefix}`,
        });
      }
    } catch (error) {
      log.error({ err: error, channelId: req.params.channelId }, 'Deposit failed');
      res.status(500).json({ error: 'Internal error', message: 'Deposit failed' });
    }
  });

  /**
   * POST /admin/channels/:channelId/close
   * Initiate channel close
   */
  router.post('/channels/:channelId/close', async (req: Request, res: Response) => {
    try {
      if (!channelManager) {
        res.status(503).json({
          error: 'Service Unavailable',
          message: 'Settlement infrastructure not enabled',
        });
        return;
      }

      const reqChannelId = req.params.channelId as string;
      const metadata = channelManager.getChannelById(reqChannelId);

      if (!metadata) {
        res.status(404).json({ error: 'Not found', message: 'Channel not found' });
        return;
      }

      const normalizedStatus = normalizeChannelStatus(metadata.status, log);
      if (normalizedStatus !== 'open' && normalizedStatus !== 'opening') {
        res.status(400).json({
          error: 'Bad request',
          message: 'Channel is not in a closeable state',
        });
        return;
      }

      const chainPrefix = metadata.chain.split(':')[0];

      if (chainPrefix === 'evm') {
        if (!paymentChannelSDK) {
          res.status(503).json({
            error: 'Service Unavailable',
            message: 'EVM settlement infrastructure not enabled',
          });
          return;
        }

        // Close channel — starts grace period for receiver to submit claims
        await paymentChannelSDK.closeChannel(reqChannelId, metadata.tokenAddress);

        metadata.status = 'closing';
        metadata.lastActivityAt = new Date();

        log.info(
          { channelId: reqChannelId, chain: chainPrefix },
          'Channel close initiated via Admin API (grace period started)'
        );

        res.json({
          channelId: reqChannelId,
          status: 'closing',
        } satisfies CloseChannelResponse);
      } else {
        res.status(400).json({
          error: 'Bad request',
          message: `Unsupported blockchain: ${chainPrefix}`,
        });
      }
    } catch (error) {
      log.error({ err: error, channelId: req.params.channelId }, 'Channel close failed');
      res.status(500).json({ error: 'Internal error', message: 'Channel close failed' });
    }
  });

  // --- Balance and Settlement State Query Endpoints (Story 21.3) ---

  /**
   * GET /admin/balances/:peerId
   * Query balance for a specific peer
   */
  router.get('/balances/:peerId', async (req: Request, res: Response) => {
    try {
      if (!accountManager) {
        res.status(503).json({
          error: 'Service Unavailable',
          message: 'Account management not enabled',
        });
        return;
      }

      const peerId = req.params.peerId as string;
      const tokenId = (req.query.tokenId as string) || (defaultSettlementTokenId ?? 'M2M');

      const balance = await accountManager.getAccountBalance(peerId, tokenId);

      const response = {
        peerId,
        balances: [
          {
            tokenId,
            debitBalance: balance.debitBalance.toString(),
            creditBalance: balance.creditBalance.toString(),
            netBalance: balance.netBalance.toString(),
          },
        ],
      } satisfies BalanceResponse;

      log.info({ peerId, tokenId }, 'Balance queried via Admin API');
      res.json(response);
    } catch (error) {
      log.error({ err: error, peerId: req.params.peerId }, 'Balance query failed');
      res.status(500).json({ error: 'Internal error', message: 'Balance query failed' });
    }
  });

  /**
   * GET /admin/settlement/states
   * Query all settlement monitor states
   */
  router.get('/settlement/states', (_req: Request, res: Response) => {
    try {
      if (!settlementMonitor) {
        res.status(503).json({
          error: 'Service Unavailable',
          message: 'Settlement monitoring not enabled',
        });
        return;
      }

      const allStates = settlementMonitor.getAllSettlementStates();
      const states: SettlementStateResponse[] = [];

      for (const [key, state] of allStates.entries()) {
        const separatorIndex = key.lastIndexOf(':');
        const peerId = key.substring(0, separatorIndex);
        const tokenId = key.substring(separatorIndex + 1);
        states.push({ peerId, tokenId, state });
      }

      log.info({ stateCount: states.length }, 'Settlement states queried via Admin API');
      res.json(states);
    } catch (error) {
      log.error({ err: error }, 'Settlement state query failed');
      res.status(500).json({ error: 'Internal error', message: 'Settlement state query failed' });
    }
  });

  /**
   * GET /admin/channels/:channelId/claims
   * Get latest claim for a channel
   */
  router.get('/channels/:channelId/claims', async (req: Request, res: Response) => {
    try {
      if (!channelManager) {
        res.status(503).json({
          error: 'Service Unavailable',
          message: 'Settlement infrastructure not enabled',
        });
        return;
      }

      if (!claimReceiver) {
        res.status(503).json({
          error: 'Service Unavailable',
          message: 'Claim receiver not enabled',
        });
        return;
      }

      const channelId = req.params.channelId as string;
      const metadata = channelManager.getChannelById(channelId);

      if (!metadata) {
        res.status(404).json({ error: 'Not found', message: 'Channel not found' });
        return;
      }

      const chainPrefix = metadata.chain.split(':')[0];
      const blockchain = chainPrefix as BlockchainType;

      const claim = await claimReceiver.getLatestVerifiedClaim(
        metadata.peerId,
        blockchain,
        channelId
      );

      if (!claim) {
        res.status(404).json({ error: 'Not found', message: 'No claims found for this channel' });
        return;
      }

      log.info({ channelId, blockchain }, 'Claim queried via Admin API');
      res.json(claim);
    } catch (error) {
      log.error({ err: error, channelId: req.params.channelId }, 'Claim query failed');
      res.status(500).json({ error: 'Internal error', message: 'Claim query failed' });
    }
  });

  // --- ILP Send Endpoint ---

  /**
   * POST /admin/ilp/send
   * Send an outbound ILP Prepare packet
   */
  const ilpSendHandler = new IlpSendHandler(packetSender ?? null, isReady ?? null, log);
  router.post('/ilp/send', ilpSendHandler.handle.bind(ilpSendHandler));

  return router;
}

/**
 * Admin API Server Configuration
 */
export interface AdminServerConfig {
  /** Port to listen on (default: 8081) */
  port?: number;

  /** Host to bind to (default: '0.0.0.0' for Docker, '127.0.0.1' for local) */
  host?: string;

  /** Optional API key for authentication */
  apiKey?: string;

  /** Enable/disable admin API (default: false) */
  enabled?: boolean;
}

// --- Payment Channel Admin API Types ---

/** Chain format: {blockchain}:{network}:{chainId} */
export const CHAIN_FORMAT_REGEX = /^evm:[a-zA-Z0-9]+:\d+$/;

/** POST /admin/channels request body */
export interface OpenChannelRequest {
  peerId: string;
  chain: string;
  token?: string;
  tokenNetwork?: string;
  initialDeposit: string;
  settlementTimeout?: number;
  /** Peer's blockchain address (e.g., EVM address). Falls back to settlementPeers if omitted. */
  peerAddress?: string;
}

/** POST /admin/channels response.
 *  Superset of agent-society's OpenChannelResult — includes `chain` and `deposit`
 *  fields that agent-society ignores but are useful for debugging.
 *  Agent-society expects: { channelId: string, status: string }
 */
export interface OpenChannelResponse {
  channelId: string;
  chain: string;
  status: AdminChannelStatus;
  deposit: string;
}

/** GET /admin/channels response item */
export interface ChannelSummary {
  channelId: string;
  peerId: string;
  chain: string;
  status: AdminChannelStatus;
  deposit: string;
  lastActivity: string;
}

/** GET /admin/channels/:channelId response.
 *  Agent-society's ChannelState expects: { channelId, status, chain }
 *  This response is a superset — additional fields (deposit, etc.) are safe to ignore.
 */
export interface ChannelDetailResponse {
  channelId: string;
  status: AdminChannelStatus;
  deposit: string;
  [key: string]: unknown;
}

/** POST /admin/channels/:channelId/deposit request body */
export interface DepositRequest {
  amount: string;
  token?: string;
}

/** POST /admin/channels/:channelId/deposit response */
export interface DepositResponse {
  channelId: string;
  /**
   * For EVM channels: total cumulative deposit from getChannelState().myDeposit (includes all prior deposits).
   */
  newDeposit: string;
  status: AdminChannelStatus;
}

/** POST /admin/channels/:channelId/close request body */
export interface CloseChannelRequest {
  cooperative?: boolean;
}

/** POST /admin/channels/:channelId/close response */
export interface CloseChannelResponse {
  channelId: string;
  status: AdminChannelStatus;
  txHash?: string;
}

/**
 * Validate a deposit request body
 * @returns Object with valid flag and optional error message
 */
export function validateDepositRequest(body: Record<string, unknown>): {
  valid: boolean;
  error?: string;
} {
  if (body.amount === undefined || body.amount === null) {
    return { valid: false, error: 'Missing amount' };
  }

  if (typeof body.amount !== 'string') {
    return { valid: false, error: 'amount must be a string' };
  }

  if (!isValidNonNegativeIntegerString(body.amount)) {
    return { valid: false, error: 'amount must be a positive integer string' };
  }

  if (body.amount === '0') {
    return { valid: false, error: 'amount must be greater than zero' };
  }

  return { valid: true };
}

/**
 * Validate settlement configuration fields.
 * @returns Error message string if invalid, or null if valid
 */
export function validateSettlementConfig(s: AdminSettlementConfig): string | null {
  const VALID_PREFERENCES = ['evm', 'any'];

  if (!s.preference || !VALID_PREFERENCES.includes(s.preference)) {
    return 'settlement.preference must be one of: evm, any';
  }

  if (s.preference === 'evm' && !s.evmAddress) {
    return 'settlement.evmAddress required when preference is evm';
  }
  if (s.preference === 'any' && !s.evmAddress) {
    return 'settlement: evmAddress required when preference is any';
  }

  if (s.evmAddress && !isValidEvmAddress(s.evmAddress)) {
    return 'settlement.evmAddress must be a valid 0x-prefixed address (42 chars)';
  }
  if (s.tokenAddress && !isValidEvmAddress(s.tokenAddress)) {
    return 'settlement.tokenAddress must be a valid 0x-prefixed address (42 chars)';
  }
  if (s.tokenNetworkAddress && !isValidEvmAddress(s.tokenNetworkAddress)) {
    return 'settlement.tokenNetworkAddress must be a valid 0x-prefixed address (42 chars)';
  }
  if (s.chainId !== undefined && (!Number.isInteger(s.chainId) || s.chainId <= 0)) {
    return 'settlement.chainId must be a positive integer';
  }
  if (s.initialDeposit !== undefined && !isValidNonNegativeIntegerString(s.initialDeposit)) {
    return 'settlement.initialDeposit must be a non-negative integer string';
  }

  return null;
}

/**
 * Validate an OpenChannelRequest body
 * @returns Object with valid flag and optional error message
 */
export function validateOpenChannelRequest(body: Record<string, unknown>): {
  valid: boolean;
  error?: string;
} {
  if (!body.peerId || typeof body.peerId !== 'string') {
    return { valid: false, error: 'Missing or invalid peerId' };
  }

  if (!body.chain || typeof body.chain !== 'string') {
    return { valid: false, error: 'Missing or invalid chain' };
  }

  if (!CHAIN_FORMAT_REGEX.test(body.chain)) {
    return {
      valid: false,
      error: `Invalid chain format: ${body.chain}. Expected {blockchain}:{network}:{chainId}`,
    };
  }

  if (body.initialDeposit === undefined || body.initialDeposit === null) {
    return { valid: false, error: 'Missing initialDeposit' };
  }

  if (typeof body.initialDeposit !== 'string') {
    return { valid: false, error: 'initialDeposit must be a string' };
  }

  if (!isValidNonNegativeIntegerString(body.initialDeposit)) {
    return { valid: false, error: 'initialDeposit must be a non-negative integer string' };
  }

  if (
    body.token !== undefined &&
    typeof body.token === 'string' &&
    !isValidEvmAddress(body.token)
  ) {
    return { valid: false, error: 'Invalid token address format' };
  }

  if (
    body.tokenNetwork !== undefined &&
    typeof body.tokenNetwork === 'string' &&
    !isValidEvmAddress(body.tokenNetwork)
  ) {
    return { valid: false, error: 'Invalid tokenNetwork address format' };
  }

  if (body.settlementTimeout !== undefined) {
    if (
      typeof body.settlementTimeout !== 'number' ||
      !Number.isInteger(body.settlementTimeout) ||
      body.settlementTimeout <= 0
    ) {
      return { valid: false, error: 'settlementTimeout must be a positive integer' };
    }
  }

  return { valid: true };
}
