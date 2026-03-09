# @crosstown/connector

[![npm](https://img.shields.io/npm/v/@crosstown/connector)](https://www.npmjs.com/package/@crosstown/connector)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](../../LICENSE)

> ILP connector node for AI agent payment networks. Routes packets, tracks balances, settles on-chain.

See the [root README](../../README.md) for conceptual overview, network architecture, and Docker deployment.

## Install

```bash
npm install @crosstown/connector
```

## Quick Start

```typescript
import { ConnectorNode, createLogger } from '@crosstown/connector';

const node = new ConnectorNode('config.yaml', createLogger('my-agent', 'info'));

node.setPacketHandler(async (request) => {
  console.log(`Received ${request.amount} tokens`);
  return { accept: true };
});

await node.start();
```

## Deployment Modes

The connector supports two deployment modes via `deploymentMode` in config:

| Mode           | Value          | How packets arrive            | How packets are sent           |
| -------------- | -------------- | ----------------------------- | ------------------------------ |
| **Embedded**   | `'embedded'`   | `setPacketHandler()` callback | `node.sendPacket()`            |
| **Standalone** | `'standalone'` | HTTP POST to BLS `handlerUrl` | HTTP POST to `/admin/ilp/send` |

When `deploymentMode` is omitted, it is inferred from `localDelivery` and `adminApi` flags.

### Embedded Mode (Recommended for Libraries)

Pass a `ConnectorConfig` object directly for programmatic usage:

```typescript
import { ConnectorNode } from '@crosstown/connector';
import type { ConnectorConfig } from '@crosstown/connector';
import pino from 'pino';

const config: ConnectorConfig = {
  nodeId: 'connector-a',
  btpServerPort: 4000,
  healthCheckPort: 8080,
  deploymentMode: 'embedded',
  adminApi: { enabled: false },
  localDelivery: { enabled: false },
  peers: [],
  routes: [],
  environment: 'production',
};

const node = new ConnectorNode(config, pino({ name: 'connector-a' }));

node.setPacketHandler(async (request) => {
  if (request.isTransit) {
    // Transit notification (fire-and-forget at intermediate hops)
    console.log(`Transit: ${request.amount} tokens → ${request.destination}`);
    return { accept: true };
  }
  // Final-hop delivery
  console.log(`Delivery: ${request.amount} tokens from ${request.sourcePeer}`);
  return { accept: true };
});

await node.start();

// Register peers dynamically at runtime
await node.registerPeer({
  id: 'connector-b',
  url: 'ws://localhost:4001',
  authToken: '',
  routes: [{ prefix: 'g.connector-b' }],
  evmAddress: '0xConnectorBAddress...', // peer's EVM settlement address
});

// Send a packet
await node.sendPacket({
  destination: 'g.connector-b.agent',
  amount: 1000n,
  executionCondition: Buffer.alloc(32),
  expiresAt: new Date(Date.now() + 30000),
  data: Buffer.from('Hello'),
});
```

### Standalone Mode

For separate-process deployments, configure `localDelivery` and `adminApi`:

```yaml
nodeId: my-connector
btpServerPort: 3000
healthCheckPort: 8080
deploymentMode: standalone
environment: production

localDelivery:
  enabled: true
  handlerUrl: http://my-bls:3100
  timeout: 5000

adminApi:
  enabled: true
  port: 8081
  apiKey: ${ADMIN_API_KEY}

peers:
  - id: peer-b
    url: ws://peer-b:3001
    authToken: ''
```

## ConnectorConfig Reference

The constructor accepts either a YAML file path or a `ConnectorConfig` object:

```typescript
const node = new ConnectorNode(config: ConnectorConfig | string, logger: Logger);
```

### Required Fields

| Field           | Type            | Description                                      |
| --------------- | --------------- | ------------------------------------------------ |
| `nodeId`        | `string`        | Unique identifier for this connector             |
| `btpServerPort` | `number`        | Port for BTP WebSocket server                    |
| `peers`         | `PeerConfig[]`  | Peer connector definitions                       |
| `routes`        | `RouteConfig[]` | Routing table entries                            |
| `environment`   | `Environment`   | `'development'` \| `'staging'` \| `'production'` |

### Optional Fields

| Field             | Type                    | Default       | Description                                    |
| ----------------- | ----------------------- | ------------- | ---------------------------------------------- |
| `deploymentMode`  | `DeploymentMode`        | inferred      | `'embedded'` \| `'standalone'`                 |
| `healthCheckPort` | `number`                | `8080`        | HTTP health endpoint port                      |
| `logLevel`        | `string`                | `'info'`      | `'debug'` \| `'info'` \| `'warn'` \| `'error'` |
| `adminApi`        | `AdminApiConfig`        | disabled      | Admin REST API settings                        |
| `localDelivery`   | `LocalDeliveryConfig`   | disabled      | HTTP forwarding to BLS                         |
| `settlement`      | `SettlementConfig`      | —             | TigerBeetle accounting config                  |
| `settlementInfra` | `SettlementInfraConfig` | —             | EVM settlement infrastructure                  |
| `blockchain`      | `BlockchainConfig`      | —             | Multi-chain EVM config (Base, Arbitrum)        |
| `security`        | `SecurityConfig`        | —             | Key management backend                         |
| `performance`     | `PerformanceConfig`     | —             | Batching, pooling, parallelization             |
| `explorer`        | `ExplorerConfig`        | enabled:3001  | Explorer UI settings                           |
| `mode`            | `string`                | `'connector'` | `'connector'` \| `'gateway'`                   |

### PeerConfig

```typescript
interface PeerConfig {
  id: string; // Unique peer identifier (referenced by routes)
  url: string; // WebSocket URL: ws://host:port or wss://host:port
  authToken: string; // Shared secret (empty string for permissionless)
  evmAddress?: string; // Peer's EVM address for settlement
}
```

### RouteConfig

```typescript
interface RouteConfig {
  prefix: string; // ILP address prefix (e.g., 'g.connector-b')
  nextHop: string; // Peer ID to forward to
  priority?: number; // Higher wins (default: 0)
}
```

## ConnectorNode Public API

### Lifecycle

| Method    | Returns         | Description                                         |
| --------- | --------------- | --------------------------------------------------- |
| `start()` | `Promise<void>` | Start BTP server, connect to peers, init settlement |
| `stop()`  | `Promise<void>` | Graceful shutdown of all connections and servers    |

### Packet Handling

| Method                             | Returns                                        | Description                            |
| ---------------------------------- | ---------------------------------------------- | -------------------------------------- |
| `setPacketHandler(handler)`        | `void`                                         | Register callback for incoming packets |
| `setLocalDeliveryHandler(handler)` | `void`                                         | Register callback for local delivery   |
| `sendPacket(params)`               | `Promise<ILPFulfillPacket \| ILPRejectPacket>` | Send ILP Prepare packet                |

### Peer Management

| Method                              | Returns                     | Description              |
| ----------------------------------- | --------------------------- | ------------------------ |
| `registerPeer(config)`              | `Promise<PeerInfo>`         | Add peer at runtime      |
| `removePeer(peerId, removeRoutes?)` | `Promise<RemovePeerResult>` | Remove peer              |
| `listPeers()`                       | `PeerInfo[]`                | List all connected peers |

### Routing

| Method                                 | Returns       | Description        |
| -------------------------------------- | ------------- | ------------------ |
| `listRoutes()`                         | `RouteInfo[]` | List routing table |
| `addRoute(prefix, nextHop, priority?)` | —             | Add a route        |
| `removeRoute(prefix)`                  | —             | Remove a route     |

### Balance & Settlement

| Method                        | Returns                       | Description               |
| ----------------------------- | ----------------------------- | ------------------------- |
| `getBalance(peerId)`          | `Promise<PeerAccountBalance>` | Query peer balances       |
| `openChannel(peerId, amount)` | `Promise<void>`               | Open EVM payment channel  |
| `getChannelState(peerId)`     | `Promise<...>`                | Get payment channel state |

### Mode Inspection

| Method                | Returns          | Description                            |
| --------------------- | ---------------- | -------------------------------------- |
| `getDeploymentMode()` | `DeploymentMode` | Returns `'embedded'` or `'standalone'` |
| `isEmbedded()`        | `boolean`        | Check if embedded mode                 |
| `isStandalone()`      | `boolean`        | Check if standalone mode               |

## BTP Authentication

### Permissionless Networks (Default)

Default mode for open networks where security is at the ILP layer:

```yaml
peers:
  - id: peer-b
    url: ws://peer-b:3001
    authToken: '' # Empty = permissionless
```

### Private Networks

Disable permissionless mode and use shared secrets:

```bash
BTP_ALLOW_NOAUTH=false
BTP_PEER_PEER_B_SECRET=secret-token
```

```yaml
peers:
  - id: peer-b
    url: ws://peer-b:3001
    authToken: secret-token
```

## Per-Hop Notification

Intermediate connectors can fire non-blocking notifications to a BLS for transit packets:

```yaml
localDelivery:
  enabled: true
  handlerUrl: http://my-bls:3100
  perHopNotification: true
```

|                  | Transit (`isTransit: true`) | Final-Hop (`isTransit` omitted)    |
| ---------------- | --------------------------- | ---------------------------------- |
| **When**         | Packet passing through      | Packet addressed to this connector |
| **BLS response** | Ignored (fire-and-forget)   | Drives ILP fulfill/reject          |
| **Blocking**     | No                          | Yes                                |
| **Use case**     | Logging, analytics          | Payment acceptance, business logic |

## Accounting Backend

### Default: In-Memory Ledger

Zero dependencies. Persists to JSON snapshots on disk.

| Variable                     | Default                       | Description               |
| ---------------------------- | ----------------------------- | ------------------------- |
| `LEDGER_SNAPSHOT_PATH`       | `./data/ledger-snapshot.json` | Snapshot file path        |
| `LEDGER_PERSIST_INTERVAL_MS` | `30000`                       | Persistence interval (ms) |

### Optional: TigerBeetle

High-performance double-entry accounting. Falls back to in-memory if connection fails.

| Variable                 | Required | Description                       |
| ------------------------ | -------- | --------------------------------- |
| `TIGERBEETLE_CLUSTER_ID` | Yes      | TigerBeetle cluster identifier    |
| `TIGERBEETLE_REPLICAS`   | Yes      | Comma-separated replica addresses |

## Settlement Infrastructure

EVM payment channels on Base L2 (and optionally Arbitrum):

| Variable                      | Description                                                   |
| ----------------------------- | ------------------------------------------------------------- |
| `SETTLEMENT_ENABLED`          | Enable automatic settlement (default: `true`)                 |
| `SETTLEMENT_THRESHOLD`        | Balance threshold to trigger settlement                       |
| `SETTLEMENT_POLLING_INTERVAL` | Polling frequency in ms (default: `30000`)                    |
| `BASE_L2_RPC_URL`             | Base L2 RPC endpoint                                          |
| `EVM_PRIVATE_KEY`             | Private key (dev only — use KMS in production)                |
| `M2M_TOKEN_ADDRESS`           | ERC-20 token contract                                         |
| `TOKEN_NETWORK_REGISTRY`      | Payment channel registry contract                             |
| `KEY_BACKEND`                 | Key management: `env` \| `aws-kms` \| `gcp-kms` \| `azure-kv` |
| `NETWORK_MODE`                | Auto-configure chain settings: `testnet` \| `mainnet`         |

### Multi-Chain Support

Configure per-chain settings via `blockchain` config:

```typescript
const config: ConnectorConfig = {
  // ...
  blockchain: {
    base: {
      enabled: true,
      rpcUrl: 'https://mainnet.base.org',
      chainId: 8453,
    },
    arbitrum: {
      enabled: true,
      rpcUrl: 'https://arb1.arbitrum.io/rpc',
      chainId: 42161,
    },
  },
};
```

## Explorer UI

Built-in real-time dashboard for packet flow, balances, and settlement monitoring.

| Variable                  | Default   | Description              |
| ------------------------- | --------- | ------------------------ |
| `EXPLORER_ENABLED`        | `true`    | Enable/disable explorer  |
| `EXPLORER_PORT`           | `3001`    | HTTP/WebSocket port      |
| `EXPLORER_RETENTION_DAYS` | `7`       | Event retention period   |
| `EXPLORER_MAX_EVENTS`     | `1000000` | Maximum events to retain |

**Endpoints:**

| Endpoint          | Description                                  |
| ----------------- | -------------------------------------------- |
| `GET /api/events` | Query historical events (supports filtering) |
| `GET /api/health` | Explorer health status                       |
| `WS /ws`          | Real-time event streaming                    |

## Admin API

REST endpoints for runtime peer/route management and ILP packet sending.

| Endpoint                       | Description           |
| ------------------------------ | --------------------- |
| `GET /admin/peers`             | List all peers        |
| `POST /admin/peers`            | Add a new peer        |
| `DELETE /admin/peers/:peerId`  | Remove a peer         |
| `GET /admin/routes`            | List routing table    |
| `POST /admin/routes`           | Add a route           |
| `DELETE /admin/routes/:prefix` | Remove a route        |
| `POST /admin/ilp/send`         | Send ILP packet       |
| `GET /admin/balances/:peerId`  | Query peer balances   |
| `GET /admin/channels`          | List payment channels |
| `POST /admin/channels`         | Open payment channel  |

### Security

| Variable                | Description                                              |
| ----------------------- | -------------------------------------------------------- |
| `ADMIN_API_KEY`         | API key (required in production unless IP allowlist set) |
| `ADMIN_API_ALLOWED_IPS` | Comma-separated IPs/CIDRs                                |
| `ADMIN_API_TRUST_PROXY` | Trust `X-Forwarded-For` (default: `false`)               |

## CLI Commands

```bash
npx connector setup              # Interactive onboarding wizard
npx connector start -c config.yaml  # Start connector
npx connector health -u http://localhost:8080  # Check health
npx connector validate config.yaml  # Validate config file
```

## Exported API

**Classes:** `ConnectorNode`, `ConfigLoader`, `ConfigurationError`, `ConnectorNotStartedError`, `RoutingTable`, `PacketHandler`, `BTPServer`, `BTPClient`, `BTPClientManager`, `AdminServer`, `AccountManager`, `SettlementMonitor`, `UnifiedSettlementExecutor`, `IlpSendHandler`

**Types:** `ConnectorConfig`, `PeerConfig`, `RouteConfig`, `SettlementConfig`, `SettlementInfraConfig`, `LocalDeliveryConfig`, `LocalDeliveryHandler`, `LocalDeliveryRequest`, `LocalDeliveryResponse`, `SendPacketParams`, `PeerRegistrationRequest`, `PeerInfo`, `PeerAccountBalance`, `RouteInfo`, `RemovePeerResult`, `IlpSendRequest`, `IlpSendResponse`, `AdminSettlementConfig`, `ChannelOpenOptions`, `ChannelMetadata`, `PaymentRequest`, `PaymentResponse`, `PaymentHandler`, `PacketSenderFn`, `IsReadyFn`, `ILPPreparePacket`, `ILPFulfillPacket`, `ILPRejectPacket`

**Utilities:** `createLogger`, `createPaymentHandlerAdapter`, `computeFulfillmentFromData`, `computeConditionFromData`, `validateIlpSendRequest`, `generatePaymentId`, `mapRejectCode`, `validateResponseData`, `REJECT_CODE_MAP`

## Package Structure

```
src/
├── core/       # ConnectorNode, PacketHandler, payment handler, local delivery
├── btp/        # BTP server and client (WebSocket peers)
├── routing/    # Routing table and prefix matching
├── settlement/ # EVM settlement, payment channels, account manager
├── http/       # Admin API, health endpoints, ILP send handler
├── explorer/   # Embedded telemetry UI server and event store
├── wallet/     # HD wallet derivation for EVM keys
├── security/   # KMS integration (AWS, Azure, GCP)
├── config/     # Configuration schema, loader, and validation
├── cli/        # CLI commands (setup, start, health, validate)
└── utils/      # Logger, OER encoding
```

## Testing

```bash
npm test                 # Unit tests
npm run test:acceptance  # Acceptance tests
```

## License

MIT — see [LICENSE](../../LICENSE).
