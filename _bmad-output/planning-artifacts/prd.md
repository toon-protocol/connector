# Crosstown Connector — Product Requirements Document (As-Built)

**Package:** `@crosstown/connector` v1.6.0
**Date:** 2026-03-09
**Status:** As-Built — Documents the current state of the system
**License:** MIT

---

## Table of Contents

- [1. Product Overview](#1-product-overview)
- [2. Goals](#2-goals)
- [3. System Capabilities](#3-system-capabilities)
- [4. Deployment Models](#4-deployment-models)
- [5. Configuration Model](#5-configuration-model)
- [6. Settlement Architecture](#6-settlement-architecture)
- [7. Protocol Compliance](#7-protocol-compliance)
- [8. Observability](#8-observability)
- [9. Security](#9-security)
- [10. Infrastructure](#10-infrastructure)
- [11. Package Structure](#11-package-structure)
- [12. Functional Requirements](#12-functional-requirements)
- [13. Non-Functional Requirements](#13-non-functional-requirements)
- [14. Out of Scope](#14-out-of-scope)

---

## 1. Product Overview

### What It Is

Crosstown Connector is a production-ready Interledger Protocol (ILP) connector designed as both a TypeScript library and CLI tool for building payment networks between AI agents and services. It routes ILP packets across a network of peers via the Bilateral Transfer Protocol (BTP) over WebSocket, tracks balances with double-entry accounting, and settles to EVM-compatible blockchains (Base L2) through on-chain payment channels.

### Who It's For

- **AI agent developers** embedding payment routing directly into TypeScript applications (ElizaOS, custom agents)
- **Platform operators** running standalone connector nodes as microservices in Docker or Kubernetes
- **Protocol engineers** building on Interledger who need a compliant, observable ILP implementation

### Core Value Proposition

A single npm package that provides ILP packet routing, BTP peer connectivity, EVM settlement with per-packet cryptographic claims, double-entry accounting, and real-time observability — deployable as an embedded library or standalone service with zero external dependencies beyond Node.js.

---

## 2. Goals

- **Route ILP packets** between agents and peers across multi-hop networks using RFC-compliant ILPv4 and BTP protocols
- **Track balances off-chain** with double-entry accounting (in-memory ledger by default, TigerBeetle optional) and enforce configurable credit limits per peer
- **Settle to Base L2** via EVM payment channels with per-packet cryptographic balance proofs, enabling trustless micropayment settlement
- **Deploy flexibly** as an in-process library (embedded mode), standalone HTTP service, or Docker container — same codebase, same configuration schema
- **Provide real-time observability** through an embedded Explorer UI with WebSocket event streaming, structured logging, and optional Prometheus/OpenTelemetry integration
- **Minimize dependencies** — the connector runs with zero external services by default (in-memory accounting, no database required); TigerBeetle and EVM settlement are opt-in

---

## 3. System Capabilities

### 3.1 ILP Packet Routing

The connector implements RFC-0027 (ILPv4) packet forwarding for PREPARE, FULFILL, and REJECT packet types. Packets are serialized using OER encoding per RFC-0030. The routing table uses longest-prefix matching on RFC-0015 ILP addresses with configurable priority for tie-breaking. Packets that cannot be routed are rejected with standard ILP error codes (F02 Unreachable, T00 Internal Error, T01 Ledger Unreachable, R02 Insufficient Timeout).

### 3.2 BTP Peer Connectivity

The Bilateral Transfer Protocol (RFC-0023) provides WebSocket-based peer-to-peer communication. The connector operates both a BTP server (accepting inbound connections) and BTP clients (initiating outbound connections to peers). Connection management includes:

- Shared-secret authentication (permissionless mode available)
- Exponential backoff retry (1s → 16s cap, max 5 retries)
- Keep-alive ping/pong (30s interval, 10s timeout)
- Automatic reconnection on connection drop
- Per-packet claim attachment via BTP `protocolData` field

### 3.3 EVM Settlement

Settlement occurs on Base L2 through a three-contract system:

| Contract               | Purpose                                                           |
| ---------------------- | ----------------------------------------------------------------- |
| `TokenNetworkRegistry` | Registry of token network instances                               |
| `TokenNetwork`         | Per-token payment channel management                              |
| `PaymentChannel`       | Bidirectional payment channels with EIP-712 signed balance proofs |

**Per-Packet Claims (Epic 31):** Each outgoing ILP packet generates a cryptographically signed balance proof (EIP-712) containing cumulative transferred amount, monotonic nonce, and channel metadata. Claims are self-describing — they embed `chainId`, `tokenNetworkAddress`, and `tokenAddress` so the receiver can verify channels dynamically on first contact without prior negotiation.

**Dynamic Channel Verification (Epic 31):** When a connector receives a claim for an unknown channel, it verifies the channel state on-chain and auto-registers the peer. This eliminates the requirement for Admin API channel pre-registration and supports unilateral channel opening.

### 3.4 Double-Entry Accounting

Two accounting backends are available:

| Backend              | Default | External Dependency | Features                                                                                                                   |
| -------------------- | ------- | ------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| **In-Memory Ledger** | Yes     | None                | O(1) balance operations, JSON snapshot persistence to disk (configurable interval, default 30s), state recovery on restart |
| **TigerBeetle**      | No      | TigerBeetle cluster | ACID-compliant, microsecond latency, transfer batching, high-throughput optimizations                                      |

Both backends implement the same interface. Per peer, the system maintains two accounts (duplex): a DEBIT account (amount peer owes us / accounts receivable) and a CREDIT account (amount we owe peer / accounts payable). Credit limits are enforceable per-peer, per-token, or globally.

### 3.5 Settlement Monitoring

The `SettlementMonitor` polls account balances at configurable intervals (default 30s) and emits `SETTLEMENT_REQUIRED` events when thresholds are exceeded. Threshold hierarchy: per-token > per-peer > default. Supports both amount-based triggers (credit balance exceeds threshold) and time-based triggers (periodic settlement interval). A state machine prevents duplicate settlement triggers: IDLE → SETTLEMENT_PENDING → SETTLEMENT_IN_PROGRESS → IDLE.

### 3.6 Admin API

An optional HTTP API for runtime management of peers, routes, channels, and balances:

| Endpoint                    | Method | Purpose                                  |
| --------------------------- | ------ | ---------------------------------------- |
| `/admin/peers`              | GET    | List all peers                           |
| `/admin/peers`              | POST   | Register new peer                        |
| `/admin/peers/:peerId`      | DELETE | Remove peer                              |
| `/admin/routes`             | GET    | List routing table                       |
| `/admin/routes`             | POST   | Add route                                |
| `/admin/routes/:prefix`     | DELETE | Remove route                             |
| `/admin/ilp/send`           | POST   | Send ILP packet (BLS outbound interface) |
| `/admin/balances/:peerId`   | GET    | Query peer account balances              |
| `/admin/channels`           | GET    | List payment channels                    |
| `/admin/channels`           | POST   | Open payment channel                     |
| `/admin/settlement/:peerId` | POST   | Trigger manual settlement                |

Protected by API key authentication (timing-safe comparison) and/or IP allowlist with CIDR notation. Disabled by default.

### 3.7 Local Delivery & Per-Hop Notifications

When the connector is the final hop for an ILP packet, it forwards the packet to an external Business Logic Server (BLS) via HTTP POST to a configurable `handlerUrl`. The BLS decides accept/reject.

**Per-Hop Notifications (Epic 30):** Intermediate connectors can notify their local BLS of transit packets via fire-and-forget HTTP POST. Failures do not block packet forwarding. This transforms the ILP routing path into a computation pipeline where each hop can observe, log, or trigger side-effects.

### 3.8 Health Checks

HTTP health endpoint (default port 8080):

| Endpoint            | Purpose                                       |
| ------------------- | --------------------------------------------- |
| `GET /health`       | Basic health status                           |
| `GET /health/ready` | Readiness probe (system initialized)          |
| `GET /health/live`  | Liveness probe (accepting traffic)            |
| `GET /metrics`      | Prometheus metrics (if observability enabled) |

Reports peer connection status, TigerBeetle connectivity (if enabled), EVM RPC connectivity (if settlement enabled), and settlement trigger queue status.

---

## 4. Deployment Models

### 4.1 Embedded Mode

The connector runs in the same process as the business logic. Packets are handled via function callbacks. No HTTP communication between connector and application.

```typescript
import { ConnectorNode } from '@crosstown/connector';

const node = new ConnectorNode(config, logger);
node.setPacketHandler(async (packet) => {
  /* handle locally */
});
await node.start();
await node.sendPacket({ destination, amount, data, executionCondition, expiresAt });
```

**Public Library API:**

| Method                                                    | Purpose                                   |
| --------------------------------------------------------- | ----------------------------------------- |
| `start()` / `stop()`                                      | Lifecycle management                      |
| `sendPacket(params)`                                      | Send outbound ILP PREPARE                 |
| `setPacketHandler(handler)`                               | Register in-process packet callback       |
| `setLocalDeliveryHandler(handler)`                        | Register function handler (bypasses HTTP) |
| `registerPeer(config)`                                    | Dynamically add a peer                    |
| `removePeer(peerId)`                                      | Remove a peer                             |
| `listPeers()`                                             | List all peers with status                |
| `getBalance(peerId)`                                      | Query peer account balance                |
| `addRoute(route)` / `removeRoute(prefix)`                 | Modify routing table                      |
| `listRoutes()`                                            | List routing table entries                |
| `openChannel(params)`                                     | Open EVM payment channel                  |
| `getChannelState(channelId)`                              | Query channel metadata                    |
| `getDeploymentMode()` / `isEmbedded()` / `isStandalone()` | Mode introspection                        |
| `getHealthStatus()`                                       | Programmatic health check                 |

**Use cases:** ElizaOS agents, monolithic applications, TypeScript services that want in-process ILP routing.

### 4.2 Standalone Mode

The connector runs as a separate process or container. Communication with the BLS occurs via HTTP:

- **Inbound:** BLS posts to connector's `/admin/ilp/send` to send packets
- **Outbound:** Connector posts to BLS's `handlerUrl` for local delivery

Admin API and local delivery are typically enabled. Explorer UI serves on its own port.

**Use cases:** Microservices, multi-language integration, Docker/Kubernetes deployments, process isolation.

### 4.3 Gateway Mode

The connector acts as a lightweight messaging gateway, connecting to a first-hop connector via BTP client.

- Configuration: `mode: 'gateway'`, `firstHopUrl`, `btpAuthToken`
- No BTP server — client-only

---

## 5. Configuration Model

Configuration is provided via YAML file (path set by `CONFIG_FILE` env var), direct TypeScript/JavaScript object (embedded mode), or environment variable fallback.

### 5.1 Core Configuration

| Field             | Type          | Required | Default     | Description                              |
| ----------------- | ------------- | -------- | ----------- | ---------------------------------------- |
| `nodeId`          | string        | Yes      | —           | Unique connector identifier              |
| `btpServerPort`   | number        | Yes      | —           | BTP WebSocket server port                |
| `environment`     | enum          | Yes      | —           | `development` / `staging` / `production` |
| `peers`           | PeerConfig[]  | Yes      | —           | Peer connection definitions              |
| `routes`          | RouteConfig[] | Yes      | —           | Initial routing table                    |
| `healthCheckPort` | number        | No       | 8080        | HTTP health endpoint port                |
| `logLevel`        | enum          | No       | `info`      | `debug` / `info` / `warn` / `error`      |
| `deploymentMode`  | enum          | No       | inferred    | `embedded` / `standalone`                |
| `mode`            | enum          | No       | `connector` | `connector` / `gateway`                  |

### 5.2 Peer Configuration

| Field        | Type   | Required | Description                                         |
| ------------ | ------ | -------- | --------------------------------------------------- |
| `id`         | string | Yes      | Unique peer identifier                              |
| `url`        | string | Yes      | WebSocket URL (`ws://` or `wss://`)                 |
| `authToken`  | string | Yes      | BTP shared secret (empty string for permissionless) |
| `evmAddress` | string | No       | Peer's EVM address for settlement                   |

### 5.3 Route Configuration

| Field      | Type   | Required | Description                                  |
| ---------- | ------ | -------- | -------------------------------------------- |
| `prefix`   | string | Yes      | ILP address prefix (RFC-0015 format)         |
| `nextHop`  | string | Yes      | Peer ID or node's own ID (local delivery)    |
| `priority` | number | No       | Route priority for tie-breaking (default: 0) |

### 5.4 Settlement Configuration

**TigerBeetle Accounting** (`settlement`):

| Field                    | Type     | Description                                                                   |
| ------------------------ | -------- | ----------------------------------------------------------------------------- |
| `enableSettlement`       | boolean  | Feature flag                                                                  |
| `connectorFeePercentage` | number   | Routing fee (0.1 = 0.1%)                                                      |
| `tigerBeetleClusterId`   | number   | 32-bit cluster ID                                                             |
| `tigerBeetleReplicas`    | string[] | Array of `host:port` replicas                                                 |
| `creditLimits`           | object   | Default limit, per-peer, per-token, global ceiling                            |
| `thresholds`             | object   | Default threshold, per-peer, per-token, time-based interval, polling interval |

**EVM Settlement Infrastructure** (`settlementInfra`):

| Field                      | Type    | Description                             |
| -------------------------- | ------- | --------------------------------------- |
| `enabled`                  | boolean | Feature flag                            |
| `privateKey`               | string  | Treasury EVM private key                |
| `rpcUrl`                   | string  | Base L2 RPC endpoint                    |
| `registryAddress`          | string  | TokenNetworkRegistry contract address   |
| `tokenAddress`             | string  | ERC-20 token contract address           |
| `threshold`                | string  | Settlement threshold (BigInt as string) |
| `pollingIntervalMs`        | number  | Balance polling interval                |
| `settlementTimeoutSecs`    | number  | Channel settlement timeout              |
| `initialDepositMultiplier` | number  | Channel deposit multiplier              |
| `ledgerSnapshotPath`       | string  | In-memory ledger snapshot file path     |
| `ledgerPersistIntervalMs`  | number  | Snapshot persistence interval           |

### 5.5 Optional Feature Configuration

| Section                  | Key Fields                                                                      | Default State       |
| ------------------------ | ------------------------------------------------------------------------------- | ------------------- |
| `adminApi`               | `enabled`, `port`, `host`, `apiKey`, `allowedIPs`, `trustProxy`                 | Disabled            |
| `localDelivery`          | `enabled`, `handlerUrl`, `timeout`, `authToken`, `perHopNotification`           | Disabled            |
| `explorer`               | `enabled`, `port`, `retentionDays`, `maxEvents`                                 | Enabled (port 3001) |
| `blockchain.base`        | `enabled`, `rpcUrl`, `chainId`, `privateKey`, `registryAddress`                 | Disabled            |
| `security.keyManagement` | `backend` (`env`/`aws-kms`/`gcp-kms`/`azure-kv`/`hsm`), backend-specific fields | `env`               |
| `performance`            | `packetProcessing`, `tigerbeetle`, `telemetry`, `connectionPools.evm`           | Defaults            |

---

## 6. Settlement Architecture

### 6.1 Payment Channel Lifecycle

```
┌──────────┐     openChannel()     ┌──────────┐     deposit()     ┌──────────┐
│  No       │ ──────────────────► │  Opened   │ ──────────────► │  Funded   │
│  Channel  │                      │           │                  │           │
└──────────┘                      └──────────┘                  └────┬──────┘
                                                                     │
                                                          Per-packet claims
                                                          (off-chain, BTP)
                                                                     │
                                                                     ▼
┌──────────┐    settlementTimeout   ┌──────────┐    closeChannel()  ┌──────────┐
│  Settled  │ ◄──────────────────── │  Closing  │ ◄──────────────── │  Active   │
│           │                       │           │                    │           │
└──────────┘                       └──────────┘                    └──────────┘
```

### 6.2 Per-Packet Claim Flow

1. Connector sends ILP PREPARE to peer via BTP
2. `PerPacketClaimService.generateClaimForPacket()` called with peer ID, token ID, and packet amount
3. Cumulative transferred amount incremented, nonce incremented (atomic under Node.js single-thread model)
4. EIP-712 balance proof signed by connector's EVM private key
5. Self-describing `EVMClaimMessage` constructed with channel metadata (`chainId`, `tokenNetworkAddress`, `tokenAddress`)
6. Claim attached to BTP `protocolData` field (protocol name: `payment-channel-claim`)
7. Claim persisted to SQLite for dispute resolution (non-blocking)
8. Telemetry event emitted (non-blocking)
9. On restart, nonce and cumulative state recovered from database — gap-free balance proof chain maintained

### 6.3 Self-Describing Claim Format

```typescript
{
  version: '1.0',
  blockchain: 'evm',
  messageId: string,              // "evm-{channelId}-{nonce}-{timestamp}"
  timestamp: ISO8601,
  senderId: string,               // Connector node ID
  channelId: string,              // bytes32 hex
  nonce: number,                  // Monotonically increasing
  transferredAmount: string,      // Cumulative (JSON for BigInt precision)
  lockedAmount: string,
  locksRoot: string,              // bytes32 Merkle root
  signature: string,              // EIP-712 signed
  signerAddress: string,          // Sender's EVM address
  chainId: number,                // Network chain ID (84532 = Base Sepolia)
  tokenNetworkAddress: string,    // TokenNetwork contract
  tokenAddress: string            // ERC-20 token contract
}
```

### 6.4 Settlement Trigger Flow

1. `SettlementMonitor` polls peer balances at configurable interval
2. When credit balance exceeds threshold → emits `SETTLEMENT_REQUIRED` event
3. `SettlementExecutor` receives event, transitions state to `IN_PROGRESS`
4. Retrieves latest per-packet claim for the peer's channel
5. Submits claim on-chain to `PaymentChannel` contract
6. On success → resets channel tracking, transitions state to `IDLE`
7. On failure → logs error, state returns to `PENDING` for retry

---

## 7. Protocol Compliance

| RFC      | Name                              | Implementation                                                                                  |
| -------- | --------------------------------- | ----------------------------------------------------------------------------------------------- |
| RFC-0027 | Interledger Protocol v4 (ILPv4)   | Full PREPARE/FULFILL/REJECT handling, standard error codes, packet expiry management            |
| RFC-0023 | Bilateral Transfer Protocol (BTP) | WebSocket transport, shared-secret auth, MESSAGE/RESPONSE/ERROR frames, protocolData extensions |
| RFC-0030 | OER Encoding                      | Variable-length integer encoding, packet serialization/deserialization                          |
| RFC-0015 | ILP Addresses                     | Hierarchical addressing, longest-prefix-match routing, address validation                       |
| RFC-0001 | Interledger Architecture          | Layered protocol design, connector role, ledger abstraction                                     |

---

## 8. Observability

### 8.1 Explorer UI

A Vue.js + Vite single-page application embedded in the connector, served on a configurable port (default 3001). Provides:

- Real-time event stream via WebSocket (`/ws`)
- Packet flow visualization with filtering
- Settlement balance monitoring
- Claim sent/received dashboard
- Peer connection status and routing table display
- Event history with configurable retention (default 7 days, max 1M events)
- REST API: `GET /api/events` for querying historical events

### 8.2 Structured Logging

Pino JSON logger with configurable log level. All packet handling, routing decisions, settlement events, and BTP connection changes emit structured log entries. Logs include `nodeId` for multi-node differentiation. Output to stdout for Docker log aggregation.

### 8.3 Telemetry Events

Event types emitted throughout the system:

- `PACKET_RECEIVED`, `PACKET_FORWARDED`, `PACKET_FULFILLED`, `PACKET_REJECTED`
- `SETTLEMENT_REQUIRED`, `SETTLEMENT_INITIATED`, `SETTLEMENT_COMPLETED`, `SETTLEMENT_FAILED`
- `CHANNEL_OPENED`, `CHANNEL_CLOSED`, `CHANNEL_SETTLED`
- `CLAIM_SENT`, `CLAIM_RECEIVED`
- `ACCOUNT_BALANCE`
- `PER_HOP_NOTIFICATION`
- Connection and error events

### 8.4 Prometheus & OpenTelemetry

Optional integration via configuration:

- **Prometheus:** Metrics endpoint at `GET /metrics` on health server
- **OpenTelemetry:** OTLP exporter for distributed tracing (configurable endpoint)

---

## 9. Security

### 9.1 Admin API Security

- **API Key:** Optional `X-Api-Key` header with timing-safe comparison
- **IP Allowlist:** CIDR notation (e.g., `10.0.0.0/8`, `127.0.0.1/32`)
- **Proxy Trust:** Optional `X-Forwarded-For` trust for reverse proxy deployments
- **Default:** Disabled — must be explicitly enabled

### 9.2 BTP Authentication

Shared-secret authentication per peer. Permissionless mode available (empty auth token). Authentication occurs during WebSocket handshake with 5-second timeout.

### 9.3 Key Management

Multi-backend key management for EVM signing:

| Backend    | Description                                               |
| ---------- | --------------------------------------------------------- |
| `env`      | Private key from environment variable or config (default) |
| `aws-kms`  | AWS Key Management Service                                |
| `gcp-kms`  | Google Cloud Key Management                               |
| `azure-kv` | Azure Key Vault                                           |
| `hsm`      | Hardware Security Module                                  |

### 9.4 Deployment Mode Restrictions

The `deploymentMode` field enforces security boundaries:

- **Embedded:** Admin API disabled, local delivery disabled, function handlers only
- **Standalone:** Admin API and local delivery can be enabled, no function handlers

---

## 10. Infrastructure

### 10.1 Docker Compose Configurations

| File                               | Purpose                         | Services                                               |
| ---------------------------------- | ------------------------------- | ------------------------------------------------------ |
| `docker-compose.yml`               | Standard 3-node linear topology | Connectors A/B/C, TigerBeetle                          |
| `docker-compose-dev.yml`           | Development environment         | Anvil (EVM), TigerBeetle                               |
| `docker-compose-evm-test.yml`      | EVM settlement testing          | Connectors, Anvil, TigerBeetle                         |
| `docker-compose-base-e2e-test.yml` | End-to-end test topology        | Full test stack                                        |
| `docker-compose-production.yml`    | Production deployment           | Connectors, Prometheus, Grafana, Jaeger, OpenTelemetry |

### 10.2 Infrastructure Services

| Service                  | Purpose                              | Port            |
| ------------------------ | ------------------------------------ | --------------- |
| **Anvil** (Foundry)      | Local EVM development node           | 8545            |
| **TigerBeetle** v0.16.68 | Accounting database                  | 3000 (internal) |
| **Prometheus**           | Metrics collection (production)      | 9090            |
| **Grafana**              | Dashboard visualization (production) | 3000            |
| **Jaeger**               | Distributed tracing (production)     | 16686           |

### 10.3 Kubernetes

Kustomize-based manifests in `k8s/connector/` with base, staging, and production overlays. Includes ConfigMap, Secret, Deployment, and Service resources.

### 10.4 Makefile Targets

| Target            | Purpose                          |
| ----------------- | -------------------------------- |
| `make dev-up`     | Start development environment    |
| `make dev-down`   | Stop services (preserve volumes) |
| `make dev-reset`  | Hard reset (delete all volumes)  |
| `make dev-logs`   | View logs (follow mode)          |
| `make dev-test`   | Run integration tests            |
| `make dev-clean`  | Deep clean                       |
| `make dev-status` | Show service status              |

---

## 11. Package Structure

```
packages/
├── connector/                  @crosstown/connector v1.6.0
│   ├── src/
│   │   ├── core/              ConnectorNode, PacketHandler
│   │   ├── btp/               BTPServer, BTPClient, BTPClientManager, claim types
│   │   ├── settlement/        PaymentChannelSDK, PerPacketClaimService, SettlementMonitor,
│   │   │                      SettlementExecutor, AccountManager, InMemoryLedgerClient,
│   │   │                      ChannelManager, ClaimSender, ClaimReceiver
│   │   ├── http/              AdminApi, AdminServer, HealthServer, IlpSendHandler
│   │   ├── routing/           RoutingTable (longest-prefix match)
│   │   ├── config/            ConfigLoader, types, validation (Zod)
│   │   ├── explorer/          EventStore, EventBroadcaster, ExplorerServer
│   │   ├── security/          KeyManager (env, AWS KMS, GCP KMS, Azure KV, HSM)
│   │   ├── wallet/            HD wallet derivation
│   │   ├── telemetry/         TelemetryEmitter, event types
│   │   └── observability/     Prometheus metrics, OpenTelemetry tracing
│   ├── explorer-ui/           Vue.js + Vite embedded Explorer
│   ├── test/                  Integration and acceptance tests
│   └── dist/                  Compiled output (lib.js entry, cli/index.js binary)
│
├── contracts/                  Foundry smart contracts
│   ├── src/                   TokenNetworkRegistry, TokenNetwork, PaymentChannel
│   ├── test/                  Solidity tests
│   ├── script/                Deployment scripts
│   └── anvil-state.json       Pre-initialized Anvil state
│
├── shared/                     @crosstown/shared — ILP types, OER encoding, type guards
├── dashboard/                  Dashboard package (deprecated)
└── faucet/                     Faucet service
```

**Build & Entry Points:**

| Entry   | Path                | Purpose                                                                      |
| ------- | ------------------- | ---------------------------------------------------------------------------- |
| Library | `dist/lib.js`       | Programmatic import (`import { ConnectorNode } from '@crosstown/connector'`) |
| CLI     | `dist/cli/index.js` | `npx connector setup` / `npx connector health` / `npx connector validate`    |
| Main    | `dist/main.js`      | Standalone server (`node dist/main.js` or `npm start`)                       |

**Node.js Requirement:** >=22.11.0

---

## 12. Functional Requirements

These requirements describe the system as currently implemented.

**FR1:** The connector implements RFC-0027 ILPv4 packet forwarding for PREPARE, FULFILL, and REJECT packet types with OER encoding per RFC-0030.

**FR2:** The connector implements RFC-0023 BTP WebSocket communication for peer-to-peer connectivity with shared-secret authentication and connection lifecycle management.

**FR3:** The connector validates ILP addresses per RFC-0015 hierarchical addressing and routes packets using longest-prefix matching with configurable priority.

**FR4:** The connector supports deployment as an embedded TypeScript library or standalone HTTP service with consistent configuration schema.

**FR5:** The connector tracks peer balances using double-entry accounting with an in-memory ledger (default) or TigerBeetle (optional) and enforces configurable credit limits per peer and per token.

**FR6:** The connector settles to Base L2 (EVM) via on-chain payment channels supporting open, deposit, close, and settle operations through the TokenNetworkRegistry/TokenNetwork/PaymentChannel contract system.

**FR7:** The connector generates self-describing, EIP-712 signed per-packet balance proofs attached to BTP protocolData, enabling the receiver to verify payment channel state dynamically without prior negotiation.

**FR8:** The connector monitors peer balances against configurable thresholds (amount-based and time-based) and automatically triggers on-chain settlement when thresholds are exceeded.

**FR9:** The connector provides an optional HTTP Admin API for runtime management of peers, routes, channels, balances, and packet sending, protected by API key and IP allowlist.

**FR10:** The connector provides an embedded Explorer UI (Vue.js) with real-time WebSocket event streaming for packet visualization, settlement monitoring, and peer status.

**FR11:** The connector emits structured JSON logs (Pino) for all ILP operations, routing decisions, settlement events, and BTP connection changes with node ID for multi-instance differentiation.

**FR12:** The connector forwards locally-addressed ILP packets to an external BLS via HTTP and supports per-hop fire-and-forget notifications at intermediate hops.

**FR13:** The connector provides HTTP health check endpoints (`/health`, `/health/ready`, `/health/live`) and optional Prometheus metrics.

**FR14:** The connector supports enterprise key management backends (environment variable, AWS KMS, GCP KMS, Azure Key Vault, HSM) for EVM signing keys.

**FR15:** The connector persists per-packet claims to SQLite for dispute resolution and recovers claim state (nonce, cumulative amounts) on restart for gap-free balance proof continuity.

**FR16:** The connector supports dynamic peer registration and removal at runtime via both programmatic API (embedded) and Admin HTTP API (standalone).

---

## 13. Non-Functional Requirements

**NFR1:** The connector runs with zero external service dependencies by default (in-memory accounting, no database). TigerBeetle, EVM settlement, and Prometheus are opt-in.

**NFR2:** All code is written in TypeScript with strict type checking. Configuration validated at startup using Zod schemas with descriptive error messages.

**NFR3:** The connector supports Node.js >=22.11.0 and deploys on Linux, macOS, and Windows via Docker or native execution.

**NFR4:** The Explorer UI updates within 100ms of event emission via WebSocket for real-time observability.

**NFR5:** The connector maintains 80%+ unit test coverage for core ILP packet handling, routing, and settlement logic. Tests run via Jest.

**NFR6:** The codebase follows conventional commits and publishes to npm as `@crosstown/connector` with dual library/CLI entry points.

**NFR7:** In-memory ledger snapshots persist to disk on configurable intervals (default 30s) and restore on restart, ensuring balance continuity across connector restarts.

**NFR8:** BTP connection failures are handled gracefully with exponential backoff retry (max 5 attempts, 1s–16s) and ILP T01 (Ledger Unreachable) error codes for unroutable packets.

**NFR9:** Per-packet claim generation is non-blocking. Claim persistence and telemetry emission do not impact packet forwarding latency.

**NFR10:** The connector supports Docker-based deployment with health check probes, multi-architecture images (amd64 + arm64), and Docker Compose orchestration for multi-node topologies.

---

## 14. Out of Scope

The following capabilities are **not** part of the current system:

- **XRP Ledger settlement** — Removed in Epic 30. Previously supported XRP payment channels.
- **Aptos settlement** — Removed in Epic 30. Previously supported Aptos Move-based payment channels.
- **Multi-chain settlement** — The connector supports Base L2 (EVM) only. Tri-chain settlement was removed.
- **SPSP/STREAM protocol** — Removed in Epic 31. Payment setup is handled externally.
- **Standalone visualization dashboard** — The original React + Cytoscape.js dashboard (Epic 3) was replaced by the embedded Vue.js Explorer UI. No separate dashboard service exists.
- **Complex topology orchestration** — Hub-spoke, mesh, 8-node, and 5-node Docker Compose configurations were removed. The system supports linear topologies and custom configurations.
- **Agent-runtime middleware** — Previously a bidirectional proxy between connector and BLS. Removed; BLS integration is now via Admin API and local delivery HTTP endpoints or embedded library API.
- **Automatic peer discovery** — Peers are statically configured or dynamically registered via API. No broadcast-based discovery protocol.
- **STREAM receipts** — Not implemented; payment proofs are handled via per-packet claims.
- **Mobile or tablet UI** — Explorer UI is desktop-first (1366x768 minimum).
