# Embedded Mode Integration Guide

This guide explains how to integrate the ILP Connector as an embedded library in your application (e.g., ElizaOS agents, monolithic applications, or any TypeScript/JavaScript runtime).

## Overview

The connector supports two deployment modes:

- **Embedded Mode** (Default) - Connector runs in-process with your business logic
- **Standalone Mode** - Connector runs as a separate process/container

### When to Use Each Mode

**Use Embedded Mode (Recommended for Most Cases):**

- ✅ TypeScript/JavaScript applications (ElizaOS, agent runtimes, web servers)
- ✅ Single-process applications (monoliths, serverless functions)
- ✅ Need sub-millisecond latency (0.1ms vs 2-5ms)
- ✅ Want type-safe config (compile-time validation)
- ✅ Prefer programmatic control (no YAML files)
- ✅ Avoid microservices complexity

**Use Standalone Mode When:**

- 🔀 Process isolation required (security/stability boundaries)
- 🔀 Polyglot systems (Python, Go, Rust, Java business logic)
- 🔀 Independent scaling (connector vs business logic)
- 🔀 Kubernetes deployments (multi-container pods)
- 🔀 Declarative config preferred (YAML + environment variables)

### Key Differences

| Aspect            | Embedded Mode  | Standalone Mode    |
| ----------------- | -------------- | ------------------ |
| **Configuration** | Config objects | YAML files         |
| **Integration**   | Function calls | HTTP APIs          |
| **Deployment**    | Single process | Separate processes |
| **Latency**       | ~0.1ms         | ~2-5ms             |
| **Type Safety**   | Compile-time   | Runtime            |
| **Use Case**      | Library usage  | Service deployment |

## Architecture

### Embedded Mode Architecture

```
┌─────────────────────────────────────────────────┐
│  Your Application Process                      │
│                                                 │
│  ┌──────────────┐        ┌─────────────────┐  │
│  │  Business    │◄──────►│  ConnectorNode  │  │
│  │  Logic       │  Direct │  (Library)      │  │
│  │              │  Calls  │                 │  │
│  └──────────────┘        └────────┬────────┘  │
│                                    │            │
│                           BTP WebSocket         │
└────────────────────────────────────┼───────────┘
                                     │
                            ┌────────▼────────┐
                            │  Peer Connector │
                            │  (External)     │
                            └─────────────────┘
```

**Key Features:**

- ✅ **Zero HTTP overhead** - Direct function calls instead of REST APIs
- ✅ **Simplified DX** - `PaymentHandler` interface (no ILP knowledge required)
- ✅ **Type safety** - Full TypeScript types for all operations
- ✅ **Direct control** - Programmatic access to all connector operations

### Standalone Mode Architecture (For Comparison)

```
┌──────────────┐                    ┌─────────────────┐
│  Business    │                    │  ConnectorNode  │
│  Logic       │                    │  (Process)      │
│  (Process)   │                    │                 │
└──────┬───────┘                    └────────┬────────┘
       │                                     │
       │ HTTP POST /handle-packet            │
       ├────────────────────────────────────►│
       │                                     │
       │◄────────────────────────────────────┤
       │ HTTP POST /admin/ilp/send           │
       │                                     │
       │                            BTP WebSocket
       │                                     │
       │                            ┌────────▼────────┐
       │                            │  Peer Connector │
       │                            └─────────────────┘
```

## Quick Start

### 1. Installation

```bash
npm install @crosstown/connector
```

### 2. Initialize Connector with Config Object

**For embedded mode, use config objects directly (no YAML files needed):**

```typescript
import { ConnectorNode } from '@crosstown/connector';
import type { ConnectorConfig } from '@crosstown/connector';
import pino from 'pino';

// Create logger
const logger = pino({ level: 'info' });

// Create config object (no YAML file needed!)
const config: ConnectorConfig = {
  nodeId: 'my-agent',
  btpServerPort: 3000,

  // Embedded mode is the default - these can be omitted
  deploymentMode: 'embedded', // Optional - inferred when APIs disabled
  adminApi: { enabled: false }, // Optional - default for embedded
  localDelivery: { enabled: false }, // Optional - default for embedded

  // Peer connections
  peers: [
    {
      id: 'connector-hub',
      url: 'ws://hub.example.com:3000',
      authToken: process.env.HUB_AUTH_TOKEN || 'secret',
    },
  ],

  // Routing table
  routes: [{ prefix: 'g.hub', nextHop: 'connector-hub', priority: 0 }],

  // Environment
  environment: 'development',
};

// Initialize connector with config object
const connector = new ConnectorNode(config, logger);

// Start connector (establishes peer connections)
await connector.start();

console.log('Connector started in embedded mode');
console.log('Mode:', connector.getDeploymentMode()); // 'embedded'
console.log('Is embedded:', connector.isEmbedded()); // true
```

**Why config objects instead of YAML?**

- ✅ Type safety (TypeScript validates at compile time)
- ✅ No file I/O (faster startup)
- ✅ Easy to modify at runtime
- ✅ Works in any environment (browser, serverless, etc.)
- ✅ No environment variable parsing needed

### 3. Register Payment Handler

The `PaymentHandler` interface provides a simplified API for handling incoming payments:

```typescript
import { PaymentHandler, PaymentRequest, PaymentResponse } from '@crosstown/connector';

const handler: PaymentHandler = async (request: PaymentRequest): Promise<PaymentResponse> => {
  console.log('Received payment:');
  console.log('  Payment ID:', request.paymentId);
  console.log('  Destination:', request.destination);
  console.log('  Amount:', request.amount);
  console.log('  Expires:', request.expiresAt);

  // Decode application data if present
  if (request.data) {
    const data = Buffer.from(request.data, 'base64').toString('utf-8');
    console.log('  Data:', data);
  }

  // Business logic - validate payment
  if (BigInt(request.amount) < 1000n) {
    return {
      accept: false,
      rejectReason: {
        code: 'invalid_amount',
        message: 'Minimum payment is 1000 units',
      },
    };
  }

  // Accept payment
  return { accept: true };
};

// Register handler
connector.setPacketHandler(handler);
```

### 4. Send Payments

Send payments using the `sendPacket()` method:

```typescript
import { ILPErrorCode } from '@crosstown/shared';
import * as crypto from 'crypto';

// Prepare packet data
const data = Buffer.from(JSON.stringify({ invoice: 'INV-001' }));
const executionCondition = crypto.createHash('sha256').update(data).digest();

// Send packet
const result = await connector.sendPacket({
  destination: 'g.hub.alice',
  amount: 5000n,
  executionCondition,
  expiresAt: new Date(Date.now() + 30000), // 30 second timeout
  data,
});

// Check result
if (result.type === 'fulfill') {
  console.log('Payment fulfilled!');
  console.log('Fulfillment:', result.fulfillment.toString('hex'));
} else {
  console.log('Payment rejected:', result.code, result.message);
}
```

## Advanced Usage

### Direct Method APIs

Embedded mode provides direct method APIs for all connector operations:

#### Peer Management

```typescript
// Register a new peer
const peerInfo = await connector.registerPeer({
  id: 'connector-alice',
  url: 'ws://alice.example.com:3000',
  authToken: 'secret-token',
  routes: [{ prefix: 'g.alice', priority: 0 }],
});

// List all peers
const peers = connector.listPeers();
console.log('Connected peers:', peers.filter((p) => p.connected).length);

// Remove a peer
await connector.removePeer('connector-alice', true); // true = remove routes too
```

#### Route Management

```typescript
// Add a static route
connector.addRoute({
  prefix: 'g.bob',
  nextHop: 'connector-hub',
  priority: 10,
});

// List all routes
const routes = connector.listRoutes();
console.log('Total routes:', routes.length);

// Remove a route
connector.removeRoute('g.bob');
```

#### Account Balances

```typescript
// Get balance for a peer (requires TigerBeetle accounting)
const balance = await connector.getBalance('connector-hub', 'ILP');
console.log('Hub balance:', balance.balances[0]);
// {
//   tokenId: 'ILP',
//   debitBalance: '500',
//   creditBalance: '1200',
//   netBalance: '700'
// }
```

#### Payment Channels

```typescript
// Open a payment channel (requires settlement infrastructure)
const channel = await connector.openChannel({
  peerId: 'connector-hub',
  chain: 'evm:base:8453',
  token: '0x1234...', // ERC-20 token address
  peerAddress: '0x5678...', // Peer's EVM address
  initialDeposit: '1000000000000000000', // 1 token (18 decimals)
  settlementTimeout: 86400, // 24 hours
});

console.log('Channel ID:', channel.channelId);
console.log('Status:', channel.status); // 'open'

// Check channel state
const state = await connector.getChannelState(channel.channelId);
console.log('Channel status:', state.status);
```

### Error Handling

#### Payment Handler Errors

The payment handler adapter automatically handles errors:

```typescript
const handler: PaymentHandler = async (request: PaymentRequest) => {
  try {
    // Your business logic
    await processPayment(request);
    return { accept: true };
  } catch (error) {
    // Adapter will catch this and return T00 (internal error)
    throw error;
  }
};

// Alternatively, return explicit rejections
const handler2: PaymentHandler = async (request: PaymentRequest) => {
  const valid = await validatePayment(request);

  if (!valid) {
    return {
      accept: false,
      rejectReason: {
        code: 'invalid_request',
        message: 'Payment validation failed',
      },
    };
  }

  return { accept: true };
};
```

**Business Error Code Mapping:**

The adapter automatically maps business error codes to ILP error codes:

| Business Code        | ILP Code | Description            |
| -------------------- | -------- | ---------------------- |
| `insufficient_funds` | `T04`    | Insufficient liquidity |
| `expired`            | `R00`    | Transfer timed out     |
| `invalid_request`    | `F00`    | Bad request            |
| `invalid_amount`     | `F03`    | Invalid amount         |
| `unexpected_payment` | `F06`    | Unrequested payment    |
| `application_error`  | `F99`    | Application error      |
| `internal_error`     | `T00`    | Internal error         |
| `timeout`            | `T00`    | Timeout                |

#### SendPacket Errors

```typescript
try {
  const result = await connector.sendPacket(params);

  if (result.type === 'reject') {
    switch (result.code) {
      case ILPErrorCode.F02_UNREACHABLE:
        console.error('Destination unreachable - check routes');
        break;
      case ILPErrorCode.R00_TRANSFER_TIMED_OUT:
        console.error('Payment timed out');
        break;
      case ILPErrorCode.T04_INSUFFICIENT_LIQUIDITY:
        console.error('Insufficient liquidity');
        break;
      default:
        console.error('Payment rejected:', result.code, result.message);
    }
  }
} catch (error) {
  if (error.message === 'Connector is not started') {
    console.error('Call connector.start() before sending packets');
  } else {
    console.error('Unexpected error:', error);
  }
}
```

## ElizaOS Integration Example

Here's a complete example of integrating the connector into an ElizaOS agent:

```typescript
import { ConnectorNode, PaymentHandler, PaymentRequest } from '@crosstown/connector';
import { Action, Plugin } from '@elizaos/core';
import pino from 'pino';

export class ILPPaymentPlugin implements Plugin {
  name = 'ilp-payment-plugin';
  description = 'ILP connector integration for ElizaOS';

  private connector: ConnectorNode | null = null;

  async initialize(runtime: any): Promise<void> {
    const logger = pino({ level: runtime.config.logLevel || 'info' });

    // Initialize connector in embedded mode with config object
    this.connector = new ConnectorNode(
      {
        nodeId: runtime.agentId,
        btpServerPort: runtime.config.ilp.btpPort,
        peers: runtime.config.ilp.peers,
        routes: runtime.config.ilp.routes,
        environment: 'development',

        // Embedded mode defaults (can be omitted)
        // deploymentMode: 'embedded', // Inferred
        // adminApi: { enabled: false },
        // localDelivery: { enabled: false },
      },
      logger
    );

    // Register payment handler
    const handler: PaymentHandler = async (request: PaymentRequest) => {
      // Notify agent runtime of incoming payment
      await runtime.emit('payment:received', {
        paymentId: request.paymentId,
        amount: request.amount,
        destination: request.destination,
        data: request.data ? Buffer.from(request.data, 'base64').toString('utf-8') : null,
      });

      // Accept payment
      return { accept: true };
    };

    this.connector.setPacketHandler(handler);

    // Start connector
    await this.connector.start();

    runtime.logger.info('ILP Connector initialized in embedded mode');
  }

  async shutdown(): Promise<void> {
    if (this.connector) {
      await this.connector.stop();
      this.connector = null;
    }
  }

  // Add actions for sending payments
  actions: Action[] = [
    {
      name: 'SEND_ILP_PAYMENT',
      similes: ['send payment', 'transfer funds', 'pay'],
      description: 'Send an ILP payment to another agent',
      validate: async (runtime, message) => {
        return message.content.destination && message.content.amount;
      },
      handler: async (runtime, message) => {
        if (!this.connector) {
          throw new Error('ILP connector not initialized');
        }

        const data = Buffer.from(JSON.stringify(message.content.data || {}));
        const condition = crypto.createHash('sha256').update(data).digest();

        const result = await this.connector.sendPacket({
          destination: message.content.destination,
          amount: BigInt(message.content.amount),
          executionCondition: condition,
          expiresAt: new Date(Date.now() + 30000),
          data,
        });

        return {
          success: result.type === 'fulfill',
          result,
        };
      },
    },
  ];
}
```

**Usage in ElizaOS configuration:**

```typescript
import { ILPPaymentPlugin } from './plugins/ilp-payment';

const runtime = new AgentRuntime({
  agentId: 'agent-alice',
  config: {
    ilp: {
      btpPort: 3000,
      peers: [{ id: 'hub', url: 'ws://hub.example.com:3000', authToken: 'secret' }],
      routes: [{ prefix: 'g.hub', nextHop: 'hub', priority: 0 }],
    },
  },
  plugins: [new ILPPaymentPlugin()],
});

await runtime.initialize();
```

## Per-Hop Notifications (Epic 30)

Enable per-hop notifications to receive transit events at intermediate hops:

```yaml
localDelivery:
  enabled: true
  handlerUrl: http://localhost:8080 # Your BLS endpoint
  perHopNotification: true # Enable transit notifications
```

**Handler receives both final-hop and transit notifications:**

```typescript
const handler: PaymentHandler = async (request: PaymentRequest) => {
  if (request.isTransit) {
    // Transit notification - packet is being forwarded through this node
    console.log('Transit packet:', request.destination, request.amount);
    // Response is ignored (fire-and-forget)
    return { accept: true };
  } else {
    // Final-hop delivery - packet is for this node
    console.log('Final delivery:', request.destination, request.amount);
    // Response determines accept/reject
    return { accept: true };
  }
};
```

**Use cases:**

- Observability - Track all packets flowing through your node
- Accounting - Log transit fees
- Analytics - Monitor payment flows
- Debugging - Trace packet routes

## Performance Considerations

### Embedded vs Standalone Mode Performance

| Metric                   | Embedded Mode            | Standalone Mode                 |
| ------------------------ | ------------------------ | ------------------------------- |
| Latency (local delivery) | ~0.1ms (function call)   | ~2-5ms (HTTP round-trip)        |
| Throughput               | Limited by JS event loop | Limited by HTTP server capacity |
| Memory overhead          | Single process           | Separate process + IPC          |
| CPU overhead             | Minimal                  | HTTP parsing + serialization    |

**Recommendation:** Use embedded mode for latency-sensitive applications where sub-millisecond response times matter.

### Optimization Tips

1. **Batch operations** - Use direct method APIs to batch peer/route registrations:

```typescript
// Good - single transaction
await Promise.all([
  connector.registerPeer(peer1),
  connector.registerPeer(peer2),
  connector.registerPeer(peer3),
]);

// Bad - sequential
await connector.registerPeer(peer1);
await connector.registerPeer(peer2);
await connector.registerPeer(peer3);
```

2. **Reuse connections** - Keep connector instance alive for the application lifetime:

```typescript
// Good - singleton pattern
class PaymentService {
  private static connector: ConnectorNode;

  static async initialize() {
    if (!this.connector) {
      this.connector = new ConnectorNode(config, logger);
      await this.connector.start();
    }
    return this.connector;
  }
}

// Bad - create/destroy per request
async function handleRequest() {
  const connector = new ConnectorNode(config, logger);
  await connector.start();
  // ... use connector
  await connector.stop();
}
```

3. **Use connection pooling** - For high-throughput scenarios, configure TigerBeetle batching:

```yaml
performance:
  tigerbeetle:
    batchSize: 100
    flushIntervalMs: 10
```

## Configuration Reference

### Minimal Embedded Mode Config

**Absolute minimum (no peers/routes):**

```typescript
import { ConnectorNode } from '@crosstown/connector';
import type { ConnectorConfig } from '@crosstown/connector';
import pino from 'pino';

const config: ConnectorConfig = {
  nodeId: 'my-agent',
  btpServerPort: 3000,
  peers: [],
  routes: [],
  environment: 'development',
};

const connector = new ConnectorNode(config, pino());
```

**All optional fields have sensible defaults:**

- `deploymentMode` → `'embedded'` (inferred when `adminApi` + `localDelivery` disabled)
- `adminApi.enabled` → `false` (no HTTP admin API)
- `localDelivery.enabled` → `false` (no HTTP local delivery)
- `healthCheckPort` → `8080`
- `explorer.enabled` → `true` (telemetry explorer on port 3001)

**With TypeScript, invalid configs are caught at compile time:**

```typescript
const config: ConnectorConfig = {
  nodeId: 'my-agent',
  btpServerPort: 3000,
  peers: [],
  routes: [],
  environment: 'development',

  // TypeScript error: 'invalidField' does not exist
  // invalidField: 'value',
};
```

### Full Embedded Mode Config

```typescript
import type { ConnectorConfig } from '@crosstown/connector';

const config: ConnectorConfig = {
  nodeId: 'my-agent',

  // Deployment mode (optional - inferred from API settings)
  deploymentMode: 'embedded',

  // BTP server
  btpServerPort: 3000,
  healthCheckPort: 8080,

  // HTTP services (disabled for embedded mode)
  adminApi: { enabled: false },
  localDelivery: { enabled: false },

  // Explorer (optional - for debugging)
  explorer: {
    enabled: true,
    port: 3001,
  },

  // Peers
  peers: [
    {
      id: 'connector-hub',
      url: 'ws://hub.example.com:3000',
      authToken: process.env.HUB_AUTH_TOKEN || 'secret',
      evmAddress: '0x1234...', // Optional - for payment channels
    },
  ],

  // Routes
  routes: [{ prefix: 'g.hub', nextHop: 'connector-hub', priority: 0 }],

  // Settlement (optional)
  settlement: {
    connectorFeePercentage: 0.1,
    enableSettlement: true,
    tigerBeetleClusterId: 0,
    tigerBeetleReplicas: ['localhost:3000'],
  },

  settlementInfra: {
    enabled: true,
    rpcUrl: 'http://localhost:8545',
    registryAddress: '0x5678...',
    tokenAddress: '0x9ABC...',
    privateKey: process.env.TREASURY_PRIVATE_KEY,
  },

  // Environment
  environment: 'development',
};
```

### Standalone Mode Config (Use YAML)

**For standalone mode, use YAML files for service deployment:**

```yaml
# connector.yaml (for standalone mode)
nodeId: my-connector
deploymentMode: standalone

# HTTP services (required for standalone)
adminApi:
  enabled: true
  port: 8081

localDelivery:
  enabled: true
  handlerUrl: http://business-logic:8080

# BTP server
btpServerPort: 3000

# Peers and routes
peers:
  - id: connector-hub
    url: ws://hub.example.com:3000
    authToken: ${HUB_AUTH_TOKEN}

routes:
  - prefix: g.hub
    nextHop: connector-hub

environment: production
```

**Load YAML config in standalone mode:**

```typescript
import { ConnectorNode } from '@crosstown/connector';
import pino from 'pino';

const logger = pino({ level: 'info' });

// Load from YAML file (standalone mode)
const connector = new ConnectorNode('./connector.yaml', logger);

await connector.start();
```

## Troubleshooting

### Common Issues

**1. Connector not started error**

```
Error: Connector is not started. Call start() before sendPacket().
```

**Solution:** Always call `await connector.start()` before using connector methods.

**2. Payment handler not receiving packets**

**Check:**

- Handler registered with `setPacketHandler()` before `start()`
- Destination address matches node's ILP address
- Routes configured correctly

**3. Admin API conflicts**

```
Warning: Admin API enabled in embedded mode - typically unnecessary
```

**Solution:** Set `adminApi.enabled: false` in config for embedded mode.

**4. Local delivery conflicts**

```
Warning: Local delivery enabled in embedded mode - use function handlers instead
```

**Solution:** Set `localDelivery.enabled: false` and use `setPacketHandler()` instead.

### Debug Logging

Enable verbose logging to troubleshoot issues:

```typescript
import pino from 'pino';

const logger = pino({
  level: 'debug',
  transport: {
    target: 'pino-pretty',
    options: { colorize: true },
  },
});

const connector = new ConnectorNode(config, logger);
```

**View packet flow:**

```
DEBUG [PacketHandler] Handling prepare packet: g.hub.alice (5000 units)
DEBUG [RoutingTable] Route lookup: g.hub.alice -> connector-hub
DEBUG [BTPClient] Sending packet to connector-hub
DEBUG [PacketHandler] Received fulfill from connector-hub
```

## Migration Guide

### From Standalone to Embedded Mode

**Before (Standalone Mode - YAML + HTTP):**

```yaml
# connector.yaml (standalone mode)
nodeId: my-connector
deploymentMode: standalone
adminApi:
  enabled: true
  port: 8081
localDelivery:
  enabled: true
  handlerUrl: http://localhost:8080
```

```typescript
// connector.ts (separate process - loads YAML)
import { ConnectorNode } from '@crosstown/connector';
const connector = new ConnectorNode('./connector.yaml', logger);
await connector.start();

// business-logic.ts (separate process - HTTP server)
app.post('/handle-packet', async (req, res) => {
  const payment = req.body;
  // Handle payment
  res.json({ accept: true });
});

// Send payments via HTTP
const response = await fetch('http://localhost:8081/admin/ilp/send', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({
    destination: 'g.alice',
    amount: '5000',
    data: btoa('{}'),
  }),
});
```

**After (Embedded Mode - Config Object + Direct Calls):**

```typescript
// app.ts (single process - config object, no YAML)
import { ConnectorNode } from '@crosstown/connector';
import type { ConnectorConfig } from '@crosstown/connector';

const config: ConnectorConfig = {
  nodeId: 'my-connector',
  btpServerPort: 3000,
  peers: [...],
  routes: [...],
  environment: 'development',
  // Embedded mode defaults (APIs disabled automatically)
};

const connector = new ConnectorNode(config, logger);

// Register handler (no HTTP server needed)
connector.setPacketHandler(async (request) => {
  // Handle payment
  return { accept: true };
});

await connector.start();

// Send payments via library call (no HTTP client needed)
const result = await connector.sendPacket({
  destination: 'g.alice',
  amount: 5000n,
  executionCondition: Buffer.from('...'),
  expiresAt: new Date(Date.now() + 30000),
});
```

**Benefits:**

- ✅ **20x lower latency** (0.1ms vs 2-5ms) - No HTTP round-trips
- ✅ **Simplified deployment** - Single process, no YAML files
- ✅ **Type-safe API** - TypeScript validates config at compile time
- ✅ **Lower memory overhead** - No HTTP server/client processes
- ✅ **Easier debugging** - Single stack trace, no cross-process boundaries

## Next Steps

- [ILP Routing Guide](./ilp-routing.md) - Learn about routing table configuration
- [Payment Channel Setup](./base-payment-channel-testing.md) - Configure payment channels for settlement
- [TigerBeetle Deployment](./tigerbeetle-deployment.md) - Set up accounting backend
- [Testing Guide](./testing-coverage-guidelines.md) - Write tests for your integration

## API Reference

### ConnectorNode

```typescript
class ConnectorNode {
  constructor(config: ConnectorConfig | string, logger: Logger);

  // Lifecycle
  start(): Promise<void>;
  stop(): Promise<void>;

  // Deployment mode
  getDeploymentMode(): DeploymentMode;
  isEmbedded(): boolean;
  isStandalone(): boolean;

  // Payment handling
  setPacketHandler(handler: PaymentHandler | null): void;
  setLocalDeliveryHandler(handler: LocalDeliveryHandler | null): void;
  sendPacket(params: SendPacketParams): Promise<ILPFulfillPacket | ILPRejectPacket>;

  // Peer management
  registerPeer(config: PeerRegistrationRequest): Promise<PeerInfo>;
  removePeer(peerId: string, removeRoutes?: boolean): Promise<RemovePeerResult>;
  listPeers(): PeerInfo[];

  // Route management
  addRoute(route: RouteInfo): void;
  removeRoute(prefix: string): void;
  listRoutes(): RouteInfo[];

  // Account management
  getBalance(peerId: string, tokenId?: string): Promise<PeerAccountBalance>;

  // Payment channels
  openChannel(params: {...}): Promise<{ channelId: string; status: string }>;
  getChannelState(channelId: string): Promise<{...}>;
}
```

### PaymentHandler

```typescript
type PaymentHandler = (request: PaymentRequest) => Promise<PaymentResponse>;

interface PaymentRequest {
  paymentId: string;
  destination: string;
  amount: string;
  expiresAt: string;
  data?: string;
  isTransit?: boolean;
}

interface PaymentResponse {
  accept: boolean;
  data?: string;
  rejectReason?: {
    code: string;
    message: string;
  };
}
```

### SendPacketParams

```typescript
interface SendPacketParams {
  destination: string;
  amount: bigint;
  executionCondition: Buffer;
  expiresAt: Date;
  data?: Buffer;
}
```

## Support

- **GitHub Issues:** [anthropics/connector/issues](https://github.com/anthropics/connector/issues)
- **Documentation:** [docs.crosstown.network](https://docs.crosstown.network)
- **Discord:** [crosstown.network/discord](https://crosstown.network/discord)
