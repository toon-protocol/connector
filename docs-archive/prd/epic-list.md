# Epic List

**Epic 1: Foundation & Core ILP Protocol Implementation**
Establish monorepo structure, implement RFC-0027 (ILPv4) packet handling and routing logic with OER encoding, and deliver basic packet forwarding capability with unit tests and logging.

**Epic 2: BTP Protocol & Multi-Node Docker Deployment**
Implement RFC-0023 BTP WebSocket communication between connectors, create Docker containerization with Compose orchestration, and enable deployment of configurable N-node networks with health checks.

**Epic 3: Real-Time Visualization Dashboard**
Build React-based network visualization showing topology and animated packet flow, implement telemetry aggregation from connector nodes, and provide interactive packet inspection capabilities.

**Epic 4: Logging, Configuration & Developer Experience**
Implement comprehensive structured logging with filterable log viewer, add support for multiple network topology configurations, create test packet sender utility, and complete documentation for user onboarding.

**Epic 5: Documentation and RFC Integration**
Create comprehensive developer documentation explaining ILP concepts and ensure all RFC references are accurate, accessible, and properly integrated into the project documentation.

**Epic 6: Settlement Foundation & Accounting**
Integrate TigerBeetle as the double-entry accounting database, build account management infrastructure to track balances and credit limits between peers, implement settlement threshold triggers, and provide dashboard visualization of account states and settlement events.

**Epic 7: Local Blockchain Development Infrastructure**
Establish local blockchain node infrastructure with Anvil (Base L2 fork), rippled (XRP Ledger standalone mode), and Aptos local testnet via Docker Compose, enabling developers to build and test payment channel smart contracts locally without testnet/mainnet dependencies.

**Epic 8: EVM Payment Channels (Base L2)**
Implement XRP-style payment channels as EVM smart contracts on Base L2, deploy payment channel infrastructure via Docker, integrate with settlement layer for automatic channel settlement, and enable instant cryptocurrency micropayments between connector peers.

**Epic 9: XRP Payment Channels**
Integrate XRP Ledger payment channels (PayChan) for settlement, implement XRP payment channel state management and claim verification, enable dual-settlement support (both EVM and XRP), and provide unified settlement API for multi-chain operations.

**Epic 10: CI/CD Pipeline Reliability & Test Quality**
Eliminate recurring CI/CD pipeline failures on epic branch pull requests by fixing test quality issues (async handling, mock coverage, timeouts), implementing pre-commit quality gates, and establishing systematic testing workflows that ensure code quality before CI execution.

**Epic 11: Packet Explorer UI**
Deliver a per-node web-based explorer interface embedded in each connector that visualizes packets and events flowing through the network in real-time. The explorer provides block explorer-style inspection capabilities for ILP packets, settlements, and payment channel activity, with full event persistence via libSQL for historical browsing and analysis.

**Epic 12: Explorer — Performance, UX & Visual Quality**
Polish the Explorer UI with performance optimizations (60fps at 1000+ events, WebSocket batching), UX improvements (keyboard shortcuts, filter persistence, responsive layout, empty states), visual quality refinements (typography audit, spacing consistency, WCAG AA contrast, animations), historical data hydration for accounts and payment channels, and a Peers & Routing Table view for network topology visibility.

**Epic 13: Aptos Payment Channels (Move Modules)**
Integrate Aptos blockchain payment channels for settlement, enabling tri-chain settlement support where connectors can settle using EVM payment channels (Epic 8), XRP payment channels (Epic 9), and Aptos Move-based payment channels. Leverages Aptos's high throughput (160,000+ TPS) and sub-second finality for micropayments.

**Epic 14: Public Testnet Integration for Tri-Chain Settlement**
Add `NETWORK_MODE=testnet/local` support for all three chains (Aptos Testnet, XRP Testnet, Base Sepolia), enabling developers to run integration tests against public testnets without local Docker container dependencies. Includes testnet URL configuration, faucet API integration, and backward-compatible local mode for offline development.

**Epic 15: Blockchain Explorer Navigation Links**
Transform static wallet addresses and transaction hashes throughout the Explorer into interactive, clickable links that open the corresponding blockchain explorer in a new tab. Implements smart address type detection (Aptos, Base Sepolia, XRP Testnet) and integrates blockchain explorer URLs into all address display components.

**Epic 16: Infrastructure Hardening & CI/CD Improvements**
Remediate infrastructure review findings including Node version alignment (Dockerfile vs package.json), multi-architecture Docker builds (amd64 + arm64), security pipeline hardening (blocking npm audit, enforced Snyk scans), production secrets management, Alertmanager configuration for notifications, and resource limits for production deployments.

**Epic 17: BTP Off-Chain Claim Exchange Protocol**
Implement standardized off-chain payment channel claim exchange via BTP protocolData for all three settlement chains (XRP, EVM/Base L2, and Aptos). Enable connectors to send cryptographically signed settlement claims to peers over the existing BTP WebSocket connection without requiring separate communication channels. Build unified claim encoding/decoding infrastructure, implement claim verification workflows, add claim persistence for dispute resolution, automatic claim redemption service, and provide comprehensive telemetry for monitoring claim exchange health across all blockchain types.

**Epic 18: Explorer UI — Network Operations Center Redesign**
Transform the Connector Explorer into a distinctive, production-grade Network Operations Center (NOC) dashboard using the frontend-design skill and Playwright MCP verification. Deliver a modern, visually striking interface with a Dashboard-first approach that emphasizes real-time ILP packet routing metrics, live packet flow visualization, and comprehensive observability across all five tabs (Dashboard, Packets, Accounts, Peers, Keys) with seamless live and historical data integration. Features deep space color palette, neon ILP packet type indicators (cyan/emerald/rose), monospace technical typography, and custom animations for a professional monitoring experience.

**Epic 19: Production Deployment Parity**
Enable TigerBeetle accounting infrastructure in the docker-compose-5-peer-multihop.yml deployment by adding the TigerBeetle service, wiring real AccountManager to replace mock implementation, and verifying that the Explorer UI Accounts tab displays real-time balance data. This epic bridges Epic 6 (backend accounting code - completed) with Epic 18 (frontend UI - completed) by activating accounting in the multi-peer test deployment.

**Epic 20: Bidirectional Agent-Runtime Middleware**
Transform agent-runtime from a one-way inbound proxy into a bidirectional middleware. Add `POST /ilp/send` so the BLS can initiate outbound ILP packets (e.g., SPSP handshakes, peer announcements). Extend `POST /admin/peers` with settlement configuration fields so the BLS can register peers with chain preferences and channel IDs. Foundational enabler for agent-society Epics 7 & 8.

**Epic 21: Payment Channel Admin APIs**
Expose payment channel management and balance query endpoints on the connector Admin API. Add `POST /admin/channels` (open), `GET /admin/channels` (list), `GET /admin/channels/:channelId` (inspect), `POST /admin/channels/:channelId/deposit` (fund), `POST /admin/channels/:channelId/close` (close), `GET /admin/balances/:peerId` (balance query), and `GET /admin/settlement/states` (settlement health). Enables the BLS to manage channels via API without direct blockchain SDK access. Required by agent-society Epics 7 & 8.

**Epic 22: Agent-Runtime Middleware Simplification**
Strip STREAM session management, SPSP HTTP endpoints, and HMAC-based fulfillment from the agent-runtime middleware, replacing the fulfillment model with simple `SHA256(data)`. Removes SessionManager, SPSPServer, and STREAM fulfillment computation. Transforms agent-runtime into a thin stateless bidirectional forwarder where the BLS (agent-society) owns all SPSP/STREAM concerns via Nostr. Implements Phase 1 of the Unified Deployment Plan.

**Epic 23: Unified Deployment Infrastructure**
Create the unified deployment infrastructure that orchestrates connector, agent-runtime middleware, and agent-society BLS containers into a single deployable stack. Delivers a 16-service Docker Compose file (`docker-compose-unified.yml`), K8s manifests for agent-society (`k8s/agent-society/`), an updated deploy script with `--unified` flag and 7-phase bootstrap verification, and environment configuration for Nostr keypairs and settlement contract addresses. Implements Phases 3-6 of the Unified Deployment Plan.

**Epic 24: Connector Library API**
Refactor `ConnectorNode` to accept a config object (not a file path), expose `sendPacket()` as a public method, add `setLocalDeliveryHandler()` for direct in-process packet delivery, and surface admin operations as callable methods. Enables `@crosstown/connector` to run embedded inside an ElizaOS Service without HTTP between components. Required for ElizaOS in-process integration.

**Epic 25: CLI/Library Separation & Lifecycle Cleanup**
Separate the CLI entrypoint from library exports, remove `process.exit()` calls and signal handlers from library code, export all types needed for in-process composition, and ensure `ConnectorNode` has clean reentrant lifecycle methods. Makes `@crosstown/connector` safe to import and embed without side effects.

**Epic 26: npm Publishing Readiness**
Prepare `@crosstown/shared` and `@crosstown/connector` for npm publication. Trim connector dependencies to minimize install footprint (core consumers pull ~5 packages instead of 30+), configure package.json for dual library/CLI usage, add publish automation with correct build ordering, and validate packages install and import correctly in a clean consumer project.

**Epic 27: Test Suite & Pre-Push Hook Optimization**
Reduce pre-push hook execution from 13+ minutes to <30 seconds by restructuring the hook to run only unit tests for changed files, scoping lint/format checks to changed files (not all 1,158 files), running lint and format in parallel, and eliminating ~20 redundant test cases across 2 deleted test files introduced in epics 24-26. Establishes a proper test pyramid: pre-push runs fast unit tests, CI runs full integration suite, nightly runs performance benchmarks. Fixes 6 currently-failing performance test suites.

**Epic 28: In-Memory Ledger — Zero-Dependency Accounting**
Replace TigerBeetle as the default accounting backend with a zero-dependency, in-memory double-entry ledger that implements the same `TigerBeetleClient` interface. Uses `Map<bigint, Account>` for O(1) balance operations, persists snapshots to disk on a configurable interval (default 30s), and restores state on restart. TigerBeetle remains available as an optional high-performance backend when explicitly configured. Eliminates the mandatory external service dependency that currently degrades the connector to a stateless packet router when TigerBeetle is unavailable.

**Epic 29: Config-Driven Settlement Infrastructure**
Move settlement keypair and infrastructure configuration from `process.env` into `ConnectorConfig` so that each `ConnectorNode` instance is fully self-contained. Adds `SettlementInfraConfig` to `ConnectorConfig` with fields for private key, RPC URL, registry address, token address, and settlement parameters. Extends `PeerConfig` with per-peer `evmAddress` field, replacing the hardcoded `PEER{1-5}_EVM_ADDRESS` loop. Uses config-first pattern with env var fallback for full backward compatibility. Eliminates the `EVM_PRIVATE_KEY` swap hack. Enables multi-node test topologies in a single process without environment variable mutation.

**Epic 30: Per-Hop BLS Notification Pipeline**
Enable every connector in an ILP packet's routing path to notify its local Business Logic Server (BLS) via a non-blocking fire-and-forget HTTP POST at intermediate hops, while preserving blocking delivery at the final hop where the BLS decides accept/reject. Adds `perHopNotification` config flag to `LocalDeliveryConfig`, `isTransit` flag to `PaymentRequest`, and telemetry emission for transit notifications. Transforms the ILP routing path into a computation pipeline where each hop can observe, log, or trigger side-effects without impacting forwarding latency or protocol compliance.

**Epic 31: Self-Describing BTP Claims & Dynamic Channel Verification**
Extend BTP claims with chain and contract coordinates (`chainId`, `tokenNetworkAddress`, `tokenAddress`) making them self-describing. Enable the connector to verify unknown payment channels dynamically on-chain on first contact and auto-register peers, eliminating the requirement for Admin API channel pre-registration. Supports Crosstown's removal of the SPSP handshake by enabling unilateral channel opening -- peers can open channels and start transacting without prior negotiation.

---

## Project Status

Epics 1-18 are **completed** or **in progress**. Epic 19 enables deployment parity. Epics 20-21 enable agent-society integration. Epics 22-23 implement the Unified Deployment Plan for full agent-runtime + agent-society integration. Epics 24-26 implement the ElizaOS integration refactoring — transforming the connector from a standalone CLI application into an importable npm library for in-process composition. Epic 27 optimizes the test suite and pre-push hook for developer velocity. Epic 28 eliminates the mandatory TigerBeetle dependency by providing a zero-dependency in-memory accounting backend as the default. Epic 29 moves settlement keypair and infrastructure configuration into `ConnectorConfig` for fully self-contained connector instances and multi-node testability. Epic 30 enables per-hop BLS notifications — transforming the ILP routing path into a computation pipeline where every connector can observe and react to transiting packets. Epic 31 extends BTP claims with self-describing chain/contract coordinates and enables dynamic on-chain channel verification, eliminating the SPSP handshake dependency and supporting unilateral channel opening. The connector is feature-complete with:

- RFC-compliant ILPv4 packet routing
- BTP WebSocket protocol for connector peering
- Tri-chain settlement (EVM, XRP, Aptos)
- TigerBeetle double-entry accounting
- Explorer UI with NOC aesthetic for professional observability
- Public testnet support for all three chains
- Off-chain claim exchange for all settlement methods
