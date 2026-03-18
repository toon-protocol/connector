# Connector

[![Version](https://img.shields.io/badge/version-1.20.0-blue.svg)](CHANGELOG.md)
[![TypeScript](https://img.shields.io/badge/TypeScript-5.3.3-blue.svg)](https://www.typescriptlang.org/)
[![License](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

> **The payment infrastructure for agent networks.** Route micropayments between autonomous agents using proven protocols. Messages carry value. Routing earns fees. Settlement happens on-chain.

## What is a Connector?

A **connector** is a node in the [Interledger Protocol (ILP)](https://interledger.org) network. Think of it as a **payment router**—the same way IP routers forward internet packets, connectors forward payment packets.

### Core Function: Routing Payments

```
Agent A sends 1000 tokens to Agent B (3 hops away)

Agent A ──► Connector 1 ──► Connector 2 ──► Agent B
  1000        (keeps 1)       (keeps 1)       gets 998

Each hop: validates, routes, earns fee
```

Connectors handle three critical tasks:

1. **Routing** — Find the path from sender to receiver using an addressing hierarchy
2. **Accounting** — Track balances with each peer off-chain (thousands of transactions)
3. **Settlement** — Periodically settle net balances on-chain (one transaction)

### How Connectors Fit in the Crosstown Stack

**Connector is the foundation. Crosstown is the application.**

```
┌─────────────────────────────────────────────────┐
│  Applications (built on connector)               │
│  ┌──────────────────────────────────────────┐   │
│  │  Crosstown                                │   │  Nostr relay with ILP payments
│  │  • Pay-to-write relay                     │   │  "Free to read, pay to write"
│  │  • Uses connector for payments            │   │
│  │  • Discovers peers via Nostr events       │   │
│  └──────────────────────────────────────────┘   │
│                                                  │
│  ┌──────────────────────────────────────────┐   │
│  │  Your Agent App                           │   │  Custom AI agent
│  │  • Business logic layer                   │   │
│  │  • Uses connector for micropayments       │   │
│  │  • Send/receive with value attached       │   │
│  └──────────────────────────────────────────┘   │
└─────────────────────────────────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────────┐
│  Connector (this repo)                           │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐        │
│  │ Routing  │ │ BTP/WS   │ │ Ledger   │        │
│  │ Table    │ │ Peers    │ │ Balances │        │
│  └──────────┘ └──────────┘ └──────────┘        │
│                                                  │
│  ┌──────────────────────────────────────────┐   │
│  │  Settlement (optional)                    │   │
│  │  • Base L2 (EVM)                          │   │
│  └──────────────────────────────────────────┘   │
└─────────────────────────────────────────────────┘
                       │
                       ▼
              ┌────────────────┐
              │  Blockchains   │  On-chain settlement
              │  (payment      │  (batched, infrequent)
              │   channels)    │
              └────────────────┘
```

**Mental Model:**

- **Connector** = Payment infrastructure (like TCP/IP for the internet)
- **Crosstown** = Application (like HTTP/HTTPS built on TCP/IP)
- **Your Agent** = Custom application (like a web browser)

The connector handles the hard parts (routing, accounting, settlement) so applications can focus on business logic.

## Install

```bash
npm install @toon-protocol/connector
```

That's it. No external databases required—the connector ships with an in-memory ledger that persists to disk via JSON snapshots. For high-throughput production workloads, you can optionally plug in [TigerBeetle](https://tigerbeetle.com).

## How Connector Networks Work

### 1. Addressing: Hierarchical ILP Addresses

Every node has an ILP address. Addresses are hierarchical, like domain names in reverse:

```
g.hub.alice       (agent "alice" on connector "hub" in global network "g")
g.hub.bob         (agent "bob" on the same connector)
g.peer.charlie    (agent "charlie" on a different connector)
```

Routing uses **longest prefix matching**:

- Traffic to `g.hub.*` → local delivery
- Traffic to `g.peer.*` → route to peer connector
- Traffic to `g.*` → route to parent connector

### 2. Peering: Bilateral Connections

Connectors peer with each other using the **Bilateral Transfer Protocol (BTP)** over WebSockets:

```yaml
# config.yaml
nodeId: hub
btpServerPort: 3000

peers:
  - id: peer
    url: ws://peer-connector:3001
    authToken: secret-token

routes:
  - prefix: g.peer
    nextHop: peer
    priority: 0
```

When you configure a peer, you're saying:

- "I trust this peer to route payments"
- "Send traffic for prefix `g.peer.*` to this peer"
- "Track balances off-chain and settle periodically"

### 3. Payment Flow: Prepare, Fulfill, Reject

ILP uses a **two-phase commit** protocol with cryptographic escrow:

```
1. PREPARE   Sender → Connectors → Receiver
   "I'll pay 1000 tokens if you provide proof X within 30 seconds"

2. FULFILL   Receiver → Connectors → Sender
   "Here's proof X (SHA256 preimage), claim your money"

3. SETTLE    Connectors update balances off-chain
```

**Key insight:** Connectors never hold funds in escrow. They track IOUs off-chain and settle the net balance on-chain when thresholds are reached.

## Quick Start

### Embedded Mode (In-Process)

**Use when:** Building an AI agent or application that needs to send/receive payments

**Benefits:**

- Zero network latency (in-process)
- Single process to manage
- Easier debugging

```typescript
import { ConnectorNode, createLogger } from '@toon-protocol/connector';

const logger = createLogger('my-agent', 'info');
const node = new ConnectorNode('config.yaml', logger);

// Handle incoming packets (same process)
node.setPacketHandler(async (request) => {
  const payload = request.data ? Buffer.from(request.data, 'base64').toString() : '';

  if (BigInt(request.amount) < 100n) {
    return {
      accept: false,
      rejectReason: { code: 'invalid_amount', message: 'Pay more' },
    };
  }

  console.log(`Received ${request.amount} tokens: ${payload}`);
  return { accept: true };
});

await node.start();

// Send a packet through the network
await node.sendPacket({
  destination: 'g.peer.agent',
  amount: 1000n,
  executionCondition: Buffer.alloc(32),
  expiresAt: new Date(Date.now() + 30000),
  data: Buffer.from('Hello, world!'),
});

await node.stop();
```

### Standalone Mode (Separate Process)

**Use when:** Running a connector as infrastructure for external applications

**Benefits:**

- Process isolation
- Independent scaling
- Language-agnostic (HTTP API)

```yaml
# config.yaml
nodeId: my-connector
btpServerPort: 3000
healthCheckPort: 8080

localDelivery:
  enabled: true
  handlerUrl: http://localhost:8080 # Your business logic server

adminApi:
  enabled: true
  port: 8081 # API for sending packets

peers:
  - id: peer-b
    url: ws://peer-b:3001
    authToken: secret-token

routes:
  - prefix: g.peer-b
    nextHop: peer-b
```

Start the connector:

```bash
npx connector start config.yaml
```

Your application receives packets via HTTP:

```typescript
// Your business logic server (separate process)
app.post('/handle-packet', async (req, res) => {
  const { paymentId, destination, amount, data } = req.body;

  if (BigInt(amount) < 100n) {
    return res.json({
      accept: false,
      rejectReason: { code: 'invalid_amount', message: 'Pay more' },
    });
  }

  res.json({ accept: true });
});
```

Send packets via HTTP API:

```bash
curl -X POST http://localhost:8081/admin/ilp/send \
  -H 'Content-Type: application/json' \
  -d '{"destination":"g.peer.agent","amount":"1000","data":"aGVsbG8="}'
```

## Configuration

### Minimal Configuration

```yaml
nodeId: my-agent
btpServerPort: 3000
healthCheckPort: 8080
logLevel: info

peers:
  - id: peer-b
    url: ws://peer-b:3001
    authToken: secret-token

routes:
  - prefix: g.peer-b
    nextHop: peer-b
    priority: 0
```

### Configuration Sections

| Section         | Purpose                                      | When to Use                   |
| --------------- | -------------------------------------------- | ----------------------------- |
| `nodeId`        | Unique identifier for this connector         | Always required               |
| `btpServerPort` | WebSocket port for incoming peer connections | Always required               |
| `peers`         | Other connectors to connect to               | Define your network topology  |
| `routes`        | Routing table (which prefixes go where)      | Map address prefixes to peers |
| `localDelivery` | Forward packets to external HTTP server      | Standalone mode only          |
| `settlement`    | On-chain settlement settings                 | When using payment channels   |
| `explorer`      | Real-time telemetry UI                       | Development and debugging     |
| `security`      | Rate limiting, IP allowlists                 | Production deployments        |
| `performance`   | Timeouts, buffer sizes                       | Performance tuning            |

**Environment Variables:**

Sensitive values (private keys, RPC URLs) are loaded from `.env`:

```bash
# Generate .env interactively
npx connector setup

# Key environment variables:
EVM_PRIVATE_KEY=0x...
EVM_RPC_URL=https://base-sepolia.g.alchemy.com/v2/...
```

See [`examples/`](examples/) for full configuration examples (linear topology, mesh, hub-spoke).

## Settlement: Batching Thousands of Payments into One Transaction

Agents exchange thousands of small payments off-chain. When thresholds are reached, the connector settles the net balance on-chain using **payment channels**.

**Example:**

```
Off-chain (fast, free):
  Agent A → Connector: 100 payments of 10 tokens = 1,000 tokens
  Connector → Agent A: 50 payments of 5 tokens = 250 tokens
  Net balance: Agent A owes 750 tokens

On-chain (slow, costs gas):
  Single transaction: Agent A → Connector: 750 tokens
```

### Supported Chains

| Chain       | Why Use It                                            | Settlement Type        |
| ----------- | ----------------------------------------------------- | ---------------------- |
| **Base L2** | Ethereum ecosystem, ERC-20 tokens, DeFi composability | Payment channels (EVM) |

Settlement is **optional**. You can run a connector without on-chain settlement for testing or private networks. The EVM settlement SDK is bundled and loaded lazily, so there's nothing extra to install.

### Payment Channels: How They Work

A payment channel is a smart contract that holds funds in escrow. Both parties can update the balance off-chain by signing claims. Only the final balance is submitted on-chain.

```
1. Open channel:
   Both parties deposit funds into a smart contract

2. Off-chain updates (thousands of transactions):
   Agent A → Connector: signed claim "I owe you 100 tokens"
   Agent A → Connector: signed claim "I owe you 200 tokens" (replaces previous)
   Agent A → Connector: signed claim "I owe you 750 tokens" (replaces previous)

3. Close channel:
   Either party submits the latest signed claim to the smart contract
   Smart contract releases funds based on the claim
```

**Key benefit:** One on-chain transaction per channel lifecycle, unlimited off-chain transactions.

## Deployment Modes

The connector supports two deployment modes via the `deploymentMode` configuration:

### `embedded` Mode (Default)

**When to use:**

- Building an AI agent with embedded connector
- Running Crosstown relay with integrated payments
- Single-process deployment

**Behavior:**

- Business logic runs in the same process
- Use `setPacketHandler()` to handle incoming payments
- No HTTP overhead
- Fastest performance

**Example:** [Crosstown relay](https://github.com/ALLiDoizCode/crosstown) uses `embedded` mode to integrate ILP payments directly into the Nostr relay.

### `standalone` Mode

**When to use:**

- Running connector as microservice infrastructure
- Process isolation between connector and business logic
- Language-agnostic integration via HTTP

**Behavior:**

- Connector forwards packets to external HTTP endpoint (`localDelivery.handlerUrl`)
- Business logic server handles packets and returns accept/reject
- Admin API enabled for sending packets via HTTP

**Example:** A Python AI agent that calls the connector's HTTP API to send payments.

## Packages

This repo is a monorepo with multiple packages:

| Package                                          | Description                                            |
| ------------------------------------------------ | ------------------------------------------------------ |
| [`@toon-protocol/connector`](packages/connector) | Connector node — routing, accounting, settlement, CLI  |
| [`@toon-protocol/shared`](packages/shared)       | Shared types and OER codec utilities                   |
| [`@toon-protocol/contracts`](packages/contracts) | EVM payment channel smart contracts (Foundry/Solidity) |
| [`@m2m-connector/faucet`](packages/faucet)       | Token faucet for local EVM testing                     |

## Explorer UI

The connector includes a built-in real-time dashboard for observability:

```yaml
explorer:
  enabled: true
  port: 3001 # or set EXPLORER_PORT env var
```

Open `http://localhost:3001` to:

- Watch packets flow through the network in real-time
- View peer balances and routing tables
- Monitor settlement events
- Debug routing decisions

Perfect for development and debugging. Disable in production.

## Example: Crosstown Integration

[Crosstown](https://github.com/ALLiDoizCode/crosstown) is a Nostr relay that uses connector as its payment layer. Crosstown is a separate project — see its repository for full integration details.

**How it works conceptually:**

1. Crosstown creates a `ConnectorNode` (payment infrastructure)
2. The relay wraps the connector with Nostr-specific logic (event storage, subscriptions, TOON encoding)
3. Events flow as ILP packets with payment attached — free to read, pay to write

**Key insight:** Crosstown doesn't implement payment routing — it delegates to connector. Connector handles routing, accounting, and settlement so applications can focus on business logic.

## Architecture: Two Modes

### Embedded Mode

```
┌─────────────────────────────────────────────┐
│  Your Agent                                  │
│  import @toon-protocol/connector             │
│  setPacketHandler() + sendPacket()           │
└──────────────────┬──────────────────────────┘
                   │ (same process)
┌──────────────────▼──────────────────────────┐
│  @toon-protocol/connector                   │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐    │
│  │ Routing  │ │ BTP/WS   │ │ Ledger   │    │
│  │ Table    │ │ Peers    │ │ Accounts │    │
│  └──────────┘ └──────────┘ └──────────┘    │
│  ┌──────────────────────────────────────┐   │
│  │  Settlement (optional)               │   │
│  └──────────┬───────────────────────────┘   │
└─────────────┼───────────────────────────────┘
              │
      ┌───────▼────────┐
      │ Base L2 (EVM)  │
      └────────────────┘
```

### Standalone Mode

```
┌──────────────┐   /handle-packet   ┌──────────────┐
│  Your BLS    │◄──────────────────│  @toon-protocol/ │
│              │                    │  connector   │
│  Outbound:   │  /admin/ilp/send  │              │
│  POST ───────│──────────────────►│              │
└──────────────┘                    └──────┬───────┘
                                           │
                                    ┌──────▼───────┐
                                    │ Blockchains  │
                                    └──────────────┘
```

## Docker Deployment

The fastest way to experiment with connectors is using Docker. Several compose files are provided at the project root:

| Compose File                            | Purpose                                  |
| --------------------------------------- | ---------------------------------------- |
| `docker-compose.yml`                    | Base multi-node network with TigerBeetle |
| `docker-compose-dev.yml`                | Development setup                        |
| `docker-compose-production.yml`         | Production deployment                    |
| `docker-compose-e2e-no-tigerbeetle.yml` | E2E tests with in-memory ledger          |
| `docker-compose-evm-test.yml`           | EVM settlement testing                   |
| `docker-compose-base-e2e-test.yml`      | Base L2 E2E testing                      |

### Quick Start: Development Network

```bash
# 1. Build the connector image
docker build -t connector .

# 2. Start the development network
docker-compose -f docker-compose-dev.yml up -d

# 3. Check status
docker-compose -f docker-compose-dev.yml ps

# 4. View logs
docker-compose -f docker-compose-dev.yml logs -f
```

### Production Deployment

```bash
# 1. Copy .env.example and configure
cp .env.example .env
# Edit .env with your settings (RPC URLs, private keys, etc.)

# 2. Start production network
docker-compose -f docker-compose-production.yml up -d

# 3. Check health
docker-compose -f docker-compose-production.yml ps
```

### TigerBeetle (High-Performance Ledger)

The base `docker-compose.yml` includes [TigerBeetle](https://tigerbeetle.com) for production-grade double-entry accounting:

```bash
# Start network with TigerBeetle
docker-compose up -d

# Check status (including TigerBeetle)
docker-compose ps

# View TigerBeetle logs
docker-compose logs -f tigerbeetle
```

### Docker Environment Variables

Configure connectors using environment variables (see `.env.example` for full list):

| Variable             | Purpose                       | Default |
| -------------------- | ----------------------------- | ------- |
| `NODE_ID`            | Connector identifier          | —       |
| `BTP_PORT`           | BTP WebSocket server port     | `4000`  |
| `HEALTH_CHECK_PORT`  | HTTP health endpoint port     | `8080`  |
| `EXPLORER_PORT`      | Explorer UI port              | `5173`  |
| `LOG_LEVEL`          | Logging verbosity             | `info`  |
| `NETWORK_MODE`       | Auto-configure chain settings | —       |
| `EVM_PRIVATE_KEY`    | Ethereum wallet private key   | —       |
| `BASE_L2_RPC_URL`    | Base L2 RPC endpoint          | —       |
| `SETTLEMENT_ENABLED` | Enable on-chain settlement    | `true`  |

### Troubleshooting

```bash
# Check container logs
docker-compose logs connector-a

# Stop and remove data (WARNING: deletes all balances)
docker-compose down -v

# Rebuild from scratch
docker build --no-cache -t connector .
```

## Development

```bash
# Clone and install
git clone https://github.com/ALLiDoizCode/connector.git
cd connector
npm install

# Build all packages
npm run build

# Run tests
npm test

# Start a local dev network (requires TigerBeetle installation)
npm run dev
```

### Requirements

- **Node.js** >= 22.11.0
- **Docker** >= 20.10.0 (for container deployments)
- **Docker Compose** >= 2.0.0

**macOS note:** For local development with TigerBeetle outside Docker, run `npm run tigerbeetle:install` first.

## Documentation

| Guide                                             | Description                          |
| ------------------------------------------------- | ------------------------------------ |
| [Configuration Examples](examples/README.md)      | YAML config schema and example files |
| [Connector Package](packages/connector/README.md) | Implementation reference and API     |
| [Changelog](CHANGELOG.md)                         | Version history and release notes    |
| [Contributing](CONTRIBUTING.md)                   | Contribution guidelines              |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## License

MIT — see [LICENSE](LICENSE).

## Links

- **GitHub:** [github.com/ALLiDoizCode/connector](https://github.com/ALLiDoizCode/connector)
- **Crosstown:** [github.com/ALLiDoizCode/crosstown](https://github.com/ALLiDoizCode/crosstown)
- **Interledger:** [interledger.org](https://interledger.org)
- **TigerBeetle:** [tigerbeetle.com](https://tigerbeetle.com)
