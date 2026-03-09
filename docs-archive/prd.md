# ILP Connector with BTP and Network Visualization - Product Requirements Document (PRD)

## Goals and Background Context

### Goals

- Provide developers and educators with a zero-config, containerized ILP network that can be deployed in under 5 minutes
- Make Interledger packet routing observable through real-time visualization and comprehensive logging
- Enable rapid experimentation with different network topologies and routing scenarios
- Reduce debugging time for ILP integration issues by 50% through enhanced visibility
- Support educational adoption of Interledger by making abstract protocol concepts tangible
- Deliver RFC-compliant ILPv4 and BTP implementations suitable for development and testing environments

### Background Context

The Interledger Protocol enables payments across different ledgers and payment networks through multi-hop routing via connector nodes. However, the current ecosystem lacks developer tools that provide visibility into packet flow and routing decisions. Existing implementations like Interledger.js focus on production functionality but offer minimal observability features, making debugging and learning challenging.

This project addresses the gap by building an observability-first ILP connector with Docker orchestration, real-time network visualization, and comprehensive logging. By containerizing multiple interconnected nodes and providing a web-based dashboard showing animated packet flow, the system will serve dual purposes: education (making ILP concepts accessible) and development (enabling efficient debugging of routing issues). The project implements RFC-0027 (ILPv4) for packet routing and RFC-0023 (BTP) for ledger-layer communication, positioning it as both a learning tool and a practical development environment for the Interledger ecosystem.

### Change Log

| Date       | Version | Description          | Author          |
| ---------- | ------- | -------------------- | --------------- |
| 2025-12-26 | 0.1     | Initial PRD creation | PM Agent (John) |

---

## Requirements

### Functional Requirements

**FR1:** The system shall implement RFC-0027 (ILPv4) compliant packet forwarding including Prepare, Fulfill, and Reject packet types with proper state management

**FR2:** The system shall implement RFC-0023 (BTP) compliant bilateral transfer protocol using WebSocket connections for connector-to-connector communication

**FR3:** The system shall validate ILP addresses according to RFC-0015 hierarchical addressing scheme before routing packets

**FR4:** The system shall encode and decode ILP packets using OER (Octet Encoding Rules) per RFC-0030

**FR5:** The system shall support Docker Compose deployment of N connector nodes (configurable from 2-10 nodes) with a single command

**FR6:** The system shall automatically configure network topology (linear chain, full mesh, or custom) based on environment variables or configuration file

**FR7:** The system shall provide a web-based visualization dashboard displaying network topology as an interactive graph

**FR8:** The system shall animate packet flow in real-time showing packet movement from source to destination through intermediate hops with color-coding by packet type (Prepare=blue, Fulfill=green, Reject=red)

**FR9:** The system shall display detailed packet information (ILP packet structure, source, destination, amount, data payload) when user clicks on a packet in the visualization

**FR10:** The system shall emit structured JSON logs for all ILP operations including packet reception, routing decisions, packet forwarding, and errors

**FR11:** The system shall log routing decision rationale (which peer was selected and why based on routing table) for each forwarded packet

**FR12:** The system shall provide a filterable log viewer in the dashboard supporting filtering by log level, connector node, and packet ID

**FR13:** The system shall maintain a routing table per connector node and use it to determine next-hop forwarding decisions

**FR14:** The system shall propagate ILP error codes correctly when packets cannot be routed or fulfilled

**FR15:** The system shall send telemetry data from all connector nodes to the visualization dashboard via WebSocket or HTTP

**FR16:** The system shall include container health checks to verify connector nodes are operational after startup

**FR17:** The system shall support basic BTP authentication using shared secrets configured via environment variables

**FR18:** The system shall provide at least two example network configurations: linear chain (3+ nodes) and full mesh (3+ nodes)

**FR19:** The system shall include a test packet sender utility that can inject ILP packets into any connector node

**FR20:** The system shall display real-time network status showing which connectors are online and which BTP connections are active

### Non-Functional Requirements

**NFR1:** The system shall deploy a 5-node network and reach fully operational state within 30 seconds on a standard development machine (8GB RAM, quad-core CPU)

**NFR2:** The visualization dashboard shall update within 100ms of packet transmission to provide real-time user experience

**NFR3:** The dashboard UI shall remain responsive (user interactions processed within 100ms) while visualizing up to 100 packets per second

**NFR4:** The system shall log 100% of ILP packets without data loss even under high packet throughput

**NFR5:** The system shall run on Linux, macOS, and Windows platforms via Docker Desktop without platform-specific configuration

**NFR6:** The visualization dashboard shall be compatible with modern browsers (Chrome 90+, Firefox 88+, Safari 14+, Edge 90+)

**NFR7:** The system shall provide clear error messages when Docker or Docker Compose is not installed or when container startup fails

**NFR8:** The codebase shall maintain at least 80% test coverage for core ILP packet handling and routing logic

**NFR9:** The project documentation shall enable a new user with Docker experience to deploy and visualize a test network within 10 minutes

**NFR10:** The system shall support up to 10 concurrent connector nodes on a standard development machine without degradation

**NFR11:** All code shall be written in TypeScript with strict type checking enabled to minimize runtime errors

**NFR12:** The system shall use semantic versioning and maintain backward compatibility for configuration files across minor versions

---

## User Interface Design Goals

### Overall UX Vision

The dashboard should feel like a "mission control" for Interledger packet flow—clean, technical, and information-dense without being overwhelming. The primary metaphor is a network monitoring tool where users observe live traffic flowing through a system. The interface prioritizes **immediate comprehension** of network state and packet movement, with progressive disclosure of detailed information on demand. Visual design should emphasize clarity and technical precision over aesthetic flourish, similar to developer tools like Chrome DevTools or network analyzers like Wireshark.

### Key Interaction Paradigms

- **Real-time observation:** Users primarily watch packet flow passively; the visualization updates automatically without requiring user action
- **Inspect-on-demand:** Clicking packets or nodes reveals detailed information in side panels or overlays without disrupting the live visualization
- **Filter and focus:** Users can filter logs and packet types to reduce noise when debugging specific scenarios
- **Configuration-first startup:** Network topology is defined via config files before launch; runtime reconfiguration is out of scope for MVP
- **Single-page application:** All functionality accessible from one dashboard view without page navigation

### Core Screens and Views

1. **Network Topology View** - Primary screen showing graph visualization of connector nodes and their BTP connections
2. **Live Packet Animation Layer** - Overlay on topology view displaying animated packets moving between nodes
3. **Packet Detail Panel** - Expandable side panel showing full ILP packet structure (triggered by clicking packet)
4. **Node Status Panel** - Info panel showing individual connector routing table, active connections, and health status (triggered by clicking node)
5. **Log Stream Viewer** - Bottom panel or separate tab displaying filterable, scrollable structured logs from all connectors
6. **Network Configuration Summary** - Header or info panel showing current topology type, number of nodes, and overall health

### Accessibility: None

MVP focuses on developer/researcher audience using modern browsers. Accessibility features (screen reader support, keyboard navigation, WCAG compliance) are deferred to post-MVP phases. Basic usability (readable fonts, sufficient color contrast for packet type differentiation) will be ensured, but formal accessibility standards are not a requirement.

### Branding

Minimal technical aesthetic with focus on functionality over brand identity. Color palette should emphasize:

- **Functional color-coding:** Blue (Prepare), Green (Fulfill), Red (Reject) for packet types
- **Neutral background:** Dark theme preferred (reduces eye strain during extended debugging sessions)
- **Monospace fonts:** For logs and packet data to align with developer tool conventions
- **Network graph styling:** Clean, minimal node/edge styling (avoid decorative elements)

No corporate branding or logo required. Project name and version displayed in header. Typography should prioritize readability for technical content (code, addresses, hex data).

### Target Device and Platforms: Web Responsive (Desktop-first)

Primary target is **desktop browsers on development machines** (1920x1080 or higher resolution). Responsive design should gracefully handle down to 1366x768 laptop screens. Mobile and tablet support explicitly out of scope for MVP—network visualization requires screen real estate and is intended for desktop debugging workflows. UI should be usable on macOS, Linux, and Windows desktop environments without platform-specific adaptations.

---

## Technical Assumptions

### Repository Structure: Monorepo

The project will use a **monorepo** structure managed with npm workspaces (or similar tooling) containing:

- `packages/connector` - ILP connector implementation with BTP plugin
- `packages/dashboard` - React-based visualization UI
- `packages/shared` - Shared TypeScript types, utilities, and ILP packet definitions
- `docker/` - Docker Compose configurations and Dockerfiles
- `examples/` - Sample topology configurations

**Rationale:** Monorepo simplifies dependency management, enables code sharing (especially TypeScript types between connector and dashboard), and streamlines the development workflow for a single developer. The brief explicitly suggests this structure in the Technical Considerations section.

### Service Architecture

**Microservices architecture within Docker containers:**

- **Connector nodes:** Multiple identical containers (one per ILP connector), each running independently
- **Dashboard service:** Single container serving the React UI and WebSocket server for telemetry aggregation
- **No shared database:** Each connector maintains in-memory state (routing tables, peer connections)
- **Communication:**
  - BTP connections between connectors (WebSocket)
  - Telemetry from connectors to dashboard (WebSocket or HTTP POST)
  - Dashboard serves UI to user's browser (HTTP)

**Rationale:** Aligns with brief's Docker-based deployment model and observability requirements. Microservices architecture allows independent scaling of connector nodes and matches the multi-node network simulation goal. In-memory state sufficient for MVP (no persistence requirement per brief).

### Testing Requirements

**Unit + Integration testing with manual testing convenience methods:**

- **Unit tests:** Jest for core ILP packet handling, routing logic, BTP message parsing (target 80% coverage per NFR8)
- **Integration tests:** Test multi-connector packet forwarding scenarios using Docker Compose test configurations
- **Manual testing utilities:** CLI tools for sending test packets, inspecting routing tables, and triggering specific scenarios
- **No E2E UI testing:** Dashboard UI verified manually (E2E test infrastructure deferred to post-MVP)
- **Docker health checks:** Built-in container health verification (FR16)

**Rationale:** Balances quality assurance with MVP timeline constraints. Unit tests protect core protocol logic (highest risk area). Integration tests validate multi-node scenarios (key differentiator). Manual testing tools support the educational use case (developers experimenting with network). Full E2E automation deferred given solo developer resource constraint.

### Additional Technical Assumptions and Requests

- **Language: TypeScript (Node.js runtime)** - Aligns with Interledger.js ecosystem, provides type safety (NFR11), and supports both backend (connector) and frontend (dashboard) development with shared types

- **Frontend Framework: React 18+** - Mature ecosystem, excellent D3.js/Cytoscape.js integration for network visualization, large community for potential contributors

- **Visualization Library: Cytoscape.js** - Purpose-built for network graphs, performant rendering, supports animated layouts and real-time updates, MIT licensed

- **UI Styling: TailwindCSS** - Utility-first approach enables rapid UI development, minimal CSS bundle size, easy dark theme implementation

- **WebSocket Library: ws (Node.js) and native WebSocket API (browser)** - Standard, lightweight, compatible with BTP WebSocket requirements from RFC-0023

- **HTTP Server: Express.js** - Minimal, well-documented, sufficient for serving dashboard static files and telemetry API endpoints

- **ILP Packet Library: Build custom implementation** - Reference existing `ilp-packet` library but implement fresh to understand RFC-0027/RFC-0030 deeply and avoid unnecessary dependencies; enables educational code walkthrough

- **Configuration Format: YAML for topology, ENV vars for runtime settings** - YAML human-readable for topology definitions (FR18), environment variables for Docker Compose integration (FR6)

- **Logging Library: Pino** - High-performance structured JSON logging, low overhead, excellent TypeScript support, aligns with FR10 structured logging requirement

- **Docker Base Images: node:20-alpine** - Small footprint, official Node.js image, Alpine Linux minimizes container size for faster startup (NFR1)

- **Version Control: Git with conventional commits** - Standard for open-source, conventional commits enable automated changelog generation (supports Change Log table requirement)

- **CI/CD: GitHub Actions** - Free for open-source, integrates with repository, supports automated testing and Docker image building

- **No database required for MVP** - All state in-memory per brief (routing tables, packet history); consider SQLite for optional packet history logging in post-MVP

- **No authentication for dashboard** - Runs on localhost, development tool, adding auth overhead not justified for MVP scope

- **Network Topology Validation:** Configuration files validated on startup with clear error messages if topology is invalid (e.g., disconnected nodes, circular references)

- **Telemetry Protocol:** Connectors push telemetry to dashboard (not pull-based); dashboard acts as aggregator with single WebSocket server accepting connections from all connectors

---

## Epic List

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

---

## Epic 1: Foundation & Core ILP Protocol Implementation

**Goal:** Establish the foundational monorepo project structure with Git, CI/CD, and development tooling, while implementing the core ILPv4 packet encoding/decoding and routing logic. This epic delivers a working single-node ILP packet processor that can parse, validate, and route packets according to RFC-0027, with comprehensive unit tests and structured logging infrastructure in place.

### Story 1.1: Initialize Monorepo with TypeScript & Development Tooling

As a developer,
I want a well-structured monorepo with TypeScript configuration, linting, and testing framework,
so that I have a solid foundation for building the ILP connector and dashboard packages.

#### Acceptance Criteria

1. Monorepo initialized with npm workspaces containing `packages/connector`, `packages/dashboard`, and `packages/shared`
2. TypeScript 5.x configured with strict mode enabled across all packages
3. ESLint and Prettier configured with shared rules for code quality and consistency
4. Jest testing framework configured with TypeScript support and coverage reporting
5. Package.json scripts support workspace commands (build, test, lint for all packages)
6. `.gitignore` properly configured to exclude node_modules, dist, and IDE-specific files
7. README.md created with project overview and setup instructions
8. Git repository initialized with conventional commit message format documented
9. GitHub Actions workflow configured to run tests and linting on pull requests
10. Project builds successfully with `npm run build` and all tests pass with `npm test`

---

### Story 1.2: Implement ILP Packet Type Definitions (TypeScript Interfaces)

As a connector developer,
I want TypeScript type definitions for all ILPv4 packet types and address formats,
so that I have type-safe representations of ILP protocol data structures.

#### Acceptance Criteria

1. TypeScript interfaces defined in `packages/shared/src/types/ilp.ts` for ILP Prepare, Fulfill, and Reject packets per RFC-0027
2. Type definitions include all required fields: amount, destination, executionCondition, expiresAt, data for Prepare packets
3. Type definitions include fulfillment field for Fulfill packets and error code/message for Reject packets
4. ILP Address type defined with validation helper functions per RFC-0015 (hierarchical addressing)
5. Packet type discriminators (Type field: 12=Prepare, 13=Fulfill, 14=Reject) defined as enums
6. Error code enum defined with all standard ILP error codes from RFC-0027 (F00-F99, T00-T99, R00-R99)
7. All types exported from `packages/shared/index.ts` for use in connector and dashboard packages
8. JSDoc comments document each type and field with references to relevant RFC sections
9. Unit tests verify type guards correctly identify packet types
10. Types compile without errors with strict TypeScript settings

---

### Story 1.3: Implement OER Encoding/Decoding for ILP Packets

As a connector developer,
I want functions to encode ILP packets to binary format and decode binary data to packet objects,
so that I can serialize packets for transmission according to RFC-0030.

#### Acceptance Criteria

1. `serializePacket()` function implemented in `packages/shared/src/encoding/oer.ts` that converts ILP packet objects to Buffer
2. `deserializePacket()` function implemented that converts Buffer to typed ILP packet objects
3. OER encoding correctly implements variable-length integer encoding per RFC-0030
4. OER encoding handles all ILP packet types (Prepare, Fulfill, Reject) correctly
5. Encoding/decoding handles edge cases: zero amounts, maximum uint64 amounts, empty data fields
6. Encoding produces binary output matching test vectors from RFC-0027 specification
7. Decoding rejects malformed packets with descriptive error messages
8. Unit tests achieve >90% coverage for encoding/decoding logic
9. Performance test validates encoding/decoding of 1000 packets completes in <100ms
10. Functions properly handle binary data fields and UTF-8 encoded strings

---

### Story 1.4: Implement In-Memory Routing Table

As a connector operator,
I want a routing table that stores destination prefixes and next-hop peer mappings,
so that the connector can determine where to forward packets.

#### Acceptance Criteria

1. `RoutingTable` class implemented in `packages/connector/src/routing/routing-table.ts`
2. Routing table supports adding routes with ILP address prefix and next-hop peer identifier
3. Routing table supports removing routes by prefix
4. `getNextHop(destination)` method returns best-match peer using longest-prefix matching per RFC-0027
5. Routing table supports overlapping prefixes and correctly returns most specific match
6. Routes can be initialized from configuration object (array of {prefix, nextHop} entries)
7. Routing table exposes method to export all current routes for inspection/debugging
8. Thread-safe operations (concurrent reads supported, writes use appropriate locking if needed)
9. Unit tests verify longest-prefix matching with complex overlapping routes
10. Unit tests verify routing table behaves correctly with empty table (no routes configured)

---

### Story 1.5: Implement Core Packet Forwarding Logic

As an ILP connector,
I want to receive ILP Prepare packets, look up routes, and forward to the appropriate next-hop peer,
so that I can route payments through the network.

#### Acceptance Criteria

1. `PacketHandler` class implemented in `packages/connector/src/core/packet-handler.ts`
2. `handlePreparePacket()` method validates packet structure and expiration time
3. Handler looks up next-hop peer using routing table based on packet destination address
4. Handler forwards valid packets to next-hop peer (integration point defined, actual sending deferred to Epic 2)
5. Handler rejects packets with expired `expiresAt` timestamp with T00 (Transfer Timed Out) error
6. Handler rejects packets with unknown destination (no route) with F02 (Unreachable) error
7. Handler decrements packet expiry by configured safety margin before forwarding
8. Handler generates ILP Reject packets with appropriate error codes per RFC-0027
9. All packet handling events emit structured log entries (using logger interface)
10. Unit tests cover happy path (successful forward) and all error cases (expired, no route, invalid packet)

---

### Story 1.6: Integrate Pino Structured Logging

As a connector operator,
I want all ILP operations logged in structured JSON format with appropriate log levels,
so that I can debug issues and monitor connector behavior.

#### Acceptance Criteria

1. Pino logger configured in `packages/connector/src/utils/logger.ts` with JSON output format
2. Logger supports log levels: DEBUG, INFO, WARN, ERROR per FR10 requirements
3. All packet handling events logged with structured fields: packetId, packetType, source, destination, amount, timestamp
4. Routing decisions logged at INFO level with fields: destination, selectedPeer, reason
5. Errors logged at ERROR level with full error details and stack traces
6. Logger includes connector node ID in all log entries for multi-node differentiation
7. Log level configurable via environment variable (default: INFO)
8. Logger outputs to stdout for Docker container log aggregation
9. Logs are valid JSON (parseable by standard JSON parsers)
10. Unit tests verify log entries contain expected structured fields for sample operations

---

### Story 1.7: Add Unit Tests for ILP Core Logic

As a developer,
I want comprehensive unit tests for packet encoding, routing, and forwarding logic,
so that I can verify RFC compliance and prevent regressions.

#### Acceptance Criteria

1. Test suite achieves >80% code coverage for `packages/shared` (encoding) and `packages/connector` (routing, forwarding)
2. Tests include RFC-0027 test vectors for packet serialization/deserialization
3. Tests verify all ILP error codes are generated correctly by packet handler
4. Tests verify routing table longest-prefix matching with at least 5 complex scenarios
5. Tests verify packet expiry validation with edge cases (expired, about to expire, far future)
6. Tests use mocked logger to verify structured log output without console noise
7. Tests run in CI pipeline and must pass before merging code
8. Each test has clear description indicating what RFC requirement or behavior it verifies
9. Tests are isolated (no shared state between tests, clean setup/teardown)
10. `npm test` executes all tests and displays coverage report summary

---

## Epic 2: BTP Protocol & Multi-Node Docker Deployment

**Goal:** Implement the Bilateral Transfer Protocol (RFC-0023) to enable WebSocket-based communication between ILP connector nodes, containerize the connector application with Docker, and create Docker Compose orchestration that deploys configurable multi-node networks. This epic delivers a fully functional N-node ILP network running in containers with BTP connections established between peers.

### Story 2.1: Implement BTP WebSocket Server

As a connector node,
I want to accept incoming BTP connections from peer connectors via WebSocket,
so that I can receive ILP packets from upstream peers.

#### Acceptance Criteria

1. `BTPServer` class implemented in `packages/connector/src/btp/btp-server.ts` using `ws` library
2. Server listens on configurable port (environment variable `BTP_SERVER_PORT`, default 3000)
3. Server implements BTP handshake per RFC-0023 (authentication with shared secret)
4. Server accepts BTP MESSAGE frames containing ILP packets
5. Server validates BTP frame format and rejects malformed frames
6. Server extracts ILP packet from BTP MESSAGE and passes to PacketHandler for processing
7. Server supports multiple concurrent peer connections
8. Server logs all BTP connection events (connect, disconnect, message received) with peer identifier
9. Server implements graceful shutdown (closes all connections on process termination)
10. Unit tests verify BTP message parsing and authentication logic using mock WebSocket connections

---

### Story 2.2: Implement BTP WebSocket Client

As a connector node,
I want to initiate BTP connections to peer connectors and send ILP packets,
so that I can forward packets to downstream peers.

#### Acceptance Criteria

1. `BTPClient` class implemented in `packages/connector/src/btp/btp-client.ts` using `ws` library
2. Client connects to peer connector using WebSocket URL from configuration
3. Client performs BTP authentication handshake with shared secret
4. Client implements `sendPacket(packet)` method that wraps ILP packet in BTP MESSAGE frame
5. Client handles connection failures and implements retry logic (exponential backoff, max 5 retries)
6. Client emits events when connection state changes (connected, disconnected, error)
7. Client logs all outbound BTP messages with destination peer identifier
8. Client supports connection keep-alive (ping/pong frames) to detect dead connections
9. Client reconnects automatically if connection drops
10. Integration test verifies BTP client can connect to BTP server and exchange packets locally

---

### Story 2.3: Integrate BTP with Packet Forwarding

As a connector,
I want the packet handler to use BTP clients to send forwarded packets to next-hop peers,
so that packets can traverse multiple connector hops.

#### Acceptance Criteria

1. Connector configuration maps peer identifiers (used in routing table) to BTP client instances
2. `PacketHandler` uses peer identifier from routing table lookup to select appropriate BTP client
3. `PacketHandler.forwardPacket()` calls `btpClient.sendPacket()` to transmit packet via BTP
4. BTP connection failures result in ILP Reject with T01 (Ledger Unreachable) error code
5. Connector initializes BTP clients on startup based on peer configuration
6. Connector initializes BTP server on startup to accept incoming connections
7. Incoming BTP packets are routed through PacketHandler and forwarded via outgoing BTP connections
8. End-to-end integration test validates packet forwarding across 3 connectors (A→B→C)
9. Logs capture full packet path including BTP send/receive events at each hop
10. Connector handles BTP connection loss gracefully (queues packets or rejects with appropriate error)

---

### Story 2.4: Create Dockerfile for Connector Application

As a DevOps engineer,
I want a Dockerfile that builds and packages the connector application,
so that I can deploy connector nodes as containers.

#### Acceptance Criteria

1. `Dockerfile` created in repository root using `node:20-alpine` base image
2. Dockerfile uses multi-stage build (builder stage compiles TypeScript, runtime stage runs compiled JavaScript)
3. Dockerfile copies only necessary files to runtime stage (dist, node_modules production deps, package.json)
4. Dockerfile exposes BTP server port (3000) for incoming connections
5. Dockerfile sets appropriate working directory and non-root user for security
6. Dockerfile includes HEALTHCHECK instruction that verifies connector process is running
7. Docker image builds successfully with `docker build -t agent-runtime .`
8. Docker image size optimized (<200MB for Alpine-based image)
9. Container starts successfully and logs appear via `docker logs`
10. Environment variables can be passed to container to configure BTP ports, peer connections, and log level

---

### Story 2.5: Create Docker Compose Configuration for Multi-Node Network

As a developer,
I want a Docker Compose file that deploys N interconnected connector nodes,
so that I can run a multi-node ILP network with one command.

#### Acceptance Criteria

1. `docker-compose.yml` created defining services for configurable number of connector nodes (3 nodes for default example)
2. Each connector service uses the same `connector` image with unique container name
3. Connector services configured with environment variables for node ID, BTP server port, and peer connection URLs
4. Network topology configured as linear chain (Node A → Node B → Node C) using routing table configuration
5. Shared secrets for BTP authentication configured via environment variables
6. Docker network created for inter-connector communication
7. Services include health checks that verify connectors are ready
8. `docker-compose up` starts all connectors and establishes BTP connections between them
9. `docker-compose logs` shows structured logs from all connector nodes
10. Documentation in README explains how to modify docker-compose.yml to change number of nodes or topology

---

### Story 2.6: Implement Configuration File Loading for Topology

As a connector operator,
I want to define routing tables and peer connections in a YAML configuration file,
so that I can easily specify network topology without modifying code.

#### Acceptance Criteria

1. `config.yaml` schema defined for specifying: node ID, BTP server port, peers (URL, auth secret), routes (prefix, nextHop)
2. Connector loads configuration from file path specified in `CONFIG_FILE` environment variable
3. Configuration parser validates all required fields are present and correctly formatted
4. Configuration parser validates that peer IDs referenced in routes exist in peers list
5. Invalid configuration results in startup failure with clear error message indicating what's wrong
6. Example configuration files provided for linear (3 nodes) and mesh (4 nodes) topologies in `examples/` directory
7. Connector initializes routing table from config routes on startup
8. Connector initializes BTP clients for all configured peers on startup
9. Configuration supports comments (YAML syntax) for documenting topology choices
10. Unit tests verify configuration parser correctly handles valid and invalid configs

---

### Story 2.7: Add Health Checks and Container Monitoring

As a DevOps engineer,
I want health check endpoints and container health status reporting,
so that I can verify connectors are operational after deployment.

#### Acceptance Criteria

1. Connector exposes HTTP health check endpoint at `/health` (simple HTTP server on separate port, default 8080)
2. Health endpoint returns 200 OK when connector is ready (BTP server listening, clients connected)
3. Health endpoint returns 503 Service Unavailable if connector is starting up or critical BTP connections are down
4. Health endpoint response includes JSON body with status, uptime, and connected peer count
5. Docker HEALTHCHECK instruction uses health endpoint to determine container health
6. `docker-compose ps` shows health status for all connector containers
7. Connectors log health check requests at DEBUG level (avoid log noise)
8. Health endpoint is accessible from Docker host for external monitoring
9. Unhealthy containers are easily identifiable in Docker Compose output
10. Integration test verifies health endpoint behavior during normal operation and simulated BTP connection failure

---

## Epic 3: Real-Time Visualization Dashboard

**Goal:** Build a React-based web dashboard that visualizes the ILP connector network as an interactive graph, displays real-time animated packet flow between nodes, and provides detailed packet inspection capabilities. This epic delivers the core observability feature that differentiates this project from production ILP implementations.

### Story 3.1: Create React Dashboard Application with Routing

As a developer,
I want a React application scaffold with routing and basic layout,
so that I can build the visualization dashboard UI.

#### Acceptance Criteria

1. React 18+ application initialized in `packages/dashboard` with TypeScript and Vite build tool
2. TailwindCSS configured for styling with dark theme as default
3. React Router configured (even if single-page for MVP, structure for future expansion)
4. Main layout component created with header (app name, version) and content area
5. Basic responsive layout works on desktop resolutions (1366x768 to 1920x1080+)
6. Application builds successfully with `npm run build` and dev server runs with `npm run dev`
7. Production build assets optimized (code splitting, minification)
8. Dashboard Dockerfile created to serve built React app via nginx or Node.js static server
9. Dashboard service added to docker-compose.yml accessible at http://localhost:8080
10. README documentation updated with instructions to access dashboard

---

### Story 3.2: Implement Network Topology Graph Visualization

As a user,
I want to see a visual graph of all connector nodes and their BTP connections,
so that I understand the network topology at a glance.

#### Acceptance Criteria

1. Cytoscape.js integrated into React dashboard for graph rendering
2. Graph displays connector nodes as labeled circles with node ID
3. Graph displays BTP connections as directed edges between nodes
4. Graph uses automatic layout algorithm (e.g., breadth-first, force-directed) to position nodes clearly
5. Graph nodes are color-coded by health status (green=healthy, yellow=degraded, red=down)
6. Graph is interactive: nodes can be dragged to reposition, zoom/pan supported
7. Graph updates when topology changes (new node appears, connection drops)
8. Graph styling follows minimal technical aesthetic (dark background, clear labels, no decorative elements)
9. Graph scales to display up to 10 nodes clearly without overlap
10. Graph renders without performance issues (smooth interactions, <100ms render time)

---

### Story 3.3: Implement Telemetry WebSocket Server in Dashboard Backend

As a dashboard,
I want to receive telemetry data from all connector nodes via WebSocket,
so that I can aggregate packet events for visualization.

#### Acceptance Criteria

1. WebSocket server implemented in `packages/dashboard/server` (or as separate package) using `ws` library
2. Server listens on configurable port (default 9000) for connector telemetry connections
3. Server accepts telemetry messages in JSON format: {type, nodeId, timestamp, data}
4. Server validates telemetry message format and logs warnings for malformed messages
5. Server supports telemetry message types: NODE_STATUS, PACKET_SENT, PACKET_RECEIVED, ROUTE_LOOKUP
6. Server broadcasts telemetry to all connected dashboard browser clients via WebSocket
7. Server handles multiple connector connections and multiple browser client connections concurrently
8. Server logs connection events (connector connected/disconnected) for debugging
9. Dashboard backend starts telemetry server on application startup
10. Integration test verifies telemetry flow from mock connector to dashboard server to browser client

---

### Story 3.4: Implement Connector Telemetry Emission

As a connector,
I want to send telemetry data about packet operations to the dashboard,
so that my activity can be visualized in real-time.

#### Acceptance Criteria

1. Connector initializes WebSocket client connection to dashboard telemetry server on startup
2. Connector sends NODE_STATUS telemetry on startup (nodeId, routes, peers, health status)
3. Connector sends PACKET_RECEIVED telemetry when BTP packet arrives (packetId, type, source, destination, amount, timestamp)
4. Connector sends ROUTE_LOOKUP telemetry when routing table lookup occurs (destination, selectedPeer, reason)
5. Connector sends PACKET_SENT telemetry when packet forwarded via BTP (packetId, nextHop, timestamp)
6. Telemetry messages include connector node ID for dashboard to differentiate sources
7. Telemetry emission is non-blocking (failures don't block packet processing)
8. Telemetry connection failures are logged but don't crash connector
9. Dashboard telemetry server URL configured via environment variable in docker-compose.yml
10. End-to-end test verifies telemetry appears in dashboard when packet flows through connector

---

### Story 3.5: Display Real-Time Packet Flow Animation

As a user,
I want to see animated visualizations of packets moving between connector nodes,
so that I can observe payment flow in real-time.

#### Acceptance Criteria

1. Dashboard listens to PACKET_SENT telemetry and creates animated packet visualization
2. Packets rendered as small colored circles moving along edges from source to destination node
3. Packet color corresponds to type: blue=Prepare, green=Fulfill, red=Reject (per FR8)
4. Packet animation duration calibrated to represent time in transit (~500ms-1s for visual clarity)
5. Multiple packets can be in flight simultaneously without visual collision
6. Packet animation smoothly interpolates position along edge (no jumpy movement)
7. Animation performance remains smooth with up to 10 concurrent packets (60fps target)
8. Packets disappear after reaching destination node (clean up to avoid visual clutter)
9. Animation uses requestAnimationFrame or CSS transitions for efficiency
10. Packet flow visualization updates in <100ms of PACKET_SENT telemetry (NFR2 requirement)

---

### Story 3.6: Implement Packet Detail Inspection Panel

As a user,
I want to click on a packet to see its full ILP packet structure and metadata,
so that I can debug packet contents and routing decisions.

#### Acceptance Criteria

1. Clicking on animated packet opens side panel with packet details
2. Detail panel displays packet ID, type, timestamp, source, destination
3. Detail panel displays ILP-specific fields: amount, executionCondition, expiresAt, data payload
4. Detail panel displays routing path (sequence of connector nodes packet has traversed)
5. Detail panel formats binary data (condition, fulfillment) as hex strings
6. Detail panel includes JSON view option showing raw packet structure
7. Detail panel closes when user clicks close button or clicks elsewhere on graph
8. Detail panel remains open while packet animates (doesn't auto-close prematurely)
9. Detail panel styled consistently with dark theme and monospace fonts for technical data
10. Multiple packet detail panels can be opened for comparison (side-by-side or stacked)

---

### Story 3.7: Implement Node Status Inspection Panel

As a user,
I want to click on a connector node to see its routing table and connection status,
so that I can understand how that node is configured.

#### Acceptance Criteria

1. Clicking on connector node in graph opens side panel with node details
2. Panel displays node ID, health status, uptime
3. Panel displays current routing table (all routes: prefix → nextHop peer)
4. Panel displays list of BTP peer connections with status (connected/disconnected)
5. Panel displays packet statistics: total packets received, forwarded, rejected
6. Panel updates in real-time as routing table or peer status changes
7. Panel includes visual indicators for health (icon or color)
8. Panel closes when user clicks close button or selects different node
9. Panel styled with monospace fonts for addresses and tabular data for routing table
10. Panel accessible even when packets are animating (doesn't interfere with click targets)

---

## Epic 4: Logging, Configuration & Developer Experience

**Goal:** Complete the developer-facing features that make the tool production-ready: comprehensive structured logging with a filterable log viewer UI, support for multiple pre-configured network topologies, a test packet sender utility for experimentation, and complete documentation. This epic transforms the working system into a polished, user-friendly development tool.

### Story 4.1: Implement Filterable Log Viewer in Dashboard

As a user,
I want to view aggregated logs from all connector nodes with filtering and search capabilities,
so that I can debug specific issues without parsing raw log files.

#### Acceptance Criteria

1. Log viewer panel added to dashboard UI (bottom panel or separate tab)
2. Dashboard telemetry server receives LOG telemetry messages from connectors containing log entries
3. Connectors emit LOG telemetry for all log entries (reuse Pino transport or duplicate to telemetry)
4. Log viewer displays log entries in reverse chronological order (newest first)
5. Log viewer supports filtering by log level (DEBUG, INFO, WARN, ERROR) with dropdown/checkboxes
6. Log viewer supports filtering by connector node ID (multi-select)
7. Log viewer supports text search/filter on log message content
8. Log viewer displays structured fields (timestamp, level, nodeId, message) in tabular format
9. Log viewer auto-scrolls to show new entries as they arrive (with option to pause auto-scroll)
10. Log viewer handles high log volume (virtualizes list rendering for >1000 entries)

---

### Story 4.2: Add Full Mesh Topology Configuration

As a developer,
I want a pre-configured full mesh network topology example,
so that I can experiment with more complex routing scenarios.

#### Acceptance Criteria

1. `examples/mesh-4-nodes.yaml` configuration file created defining 4-node full mesh topology
2. Each node in mesh has routes to all other nodes (direct connections, no multi-hop)
3. Routing tables configured so each node can reach any other node in one hop
4. BTP peer connections configured bidirectionally between all node pairs
5. Shared secrets configured for all peer connections
6. `docker-compose-mesh.yml` created that uses mesh configuration
7. `docker-compose -f docker-compose-mesh.yml up` deploys 4-node mesh successfully
8. Documentation in README explains mesh topology and how to run it
9. Mesh network visualized correctly in dashboard (all nodes interconnected)
10. Test packet successfully routed between any two nodes in mesh topology

---

### Story 4.3: Add Custom Topology Configuration Support

As a developer,
I want to define my own custom network topologies via configuration file,
so that I can test specific routing scenarios relevant to my use case.

#### Acceptance Criteria

1. Configuration file format supports arbitrary topologies (any number of nodes, any connection pattern)
2. Configuration validation detects and reports errors: disconnected nodes, invalid peer references, circular route dependencies
3. Example custom topology provided: hub-and-spoke (1 central hub, 3 spoke nodes)
4. Documentation explains configuration schema with annotated examples
5. Docker Compose configuration can be generated from topology config (or environment variables reference config)
6. Custom topology loads successfully and establishes all specified BTP connections
7. Routing table validation warns if destination is unreachable from source node
8. Topology changes can be made by editing config file and restarting containers
9. Dashboard correctly visualizes custom topologies (no hard-coded layout assumptions)
10. Complex topology (8+ nodes, 12+ connections) can be configured and deployed successfully

---

### Story 4.4: Create Test Packet Sender Utility

As a developer,
I want a CLI tool to inject test ILP packets into the network,
so that I can observe packet routing without external dependencies.

#### Acceptance Criteria

1. CLI tool `send-packet.js` created in `tools/` directory (or as npm script)
2. Tool accepts arguments: source node, destination address, amount, optional data payload
3. Tool connects to specified connector's BTP interface and sends ILP Prepare packet
4. Tool generates valid ILP packet with appropriate expiry timestamp and execution condition
5. Tool logs packet ID and confirmation of send
6. Tool supports sending multiple packets in sequence or batch mode
7. Tool can send Fulfill or Reject packets for testing error handling
8. Tool includes examples in `--help` output
9. README documents how to use tool to test different routing scenarios
10. Integration test verifies tool successfully sends packet through 3-node network and observes in dashboard

---

### Story 4.5: Add Comprehensive README Documentation

As a new user,
I want clear documentation explaining how to set up and use the ILP connector network,
so that I can get started quickly without prior knowledge of the codebase.

#### Acceptance Criteria

1. README includes project overview explaining purpose and key features
2. README includes prerequisites section (Docker, Docker Compose, Node.js versions)
3. README includes quick start guide: clone repo, run docker-compose up, access dashboard (target <10 minutes)
4. README explains how to access dashboard (URL) and what to expect
5. README documents example topologies (linear, mesh) and how to switch between them
6. README includes section on using test packet sender tool with examples
7. README includes troubleshooting section (common errors, how to view logs, health check)
8. README includes architecture overview diagram (high-level component interaction)
9. README includes links to relevant Interledger RFCs for learning more
10. README includes contributing guidelines and license information

---

### Story 4.6: Add Architecture Documentation

As a developer or contributor,
I want architecture documentation explaining how the system components interact,
so that I can understand the codebase and contribute effectively.

#### Acceptance Criteria

1. `docs/architecture.md` created explaining system architecture
2. Documentation includes component diagram showing: connector nodes, BTP connections, dashboard, telemetry flow
3. Documentation explains data flow: packet ingress → routing → BTP send → telemetry → dashboard visualization
4. Documentation explains monorepo structure and package responsibilities
5. Documentation explains configuration loading and precedence (env vars vs. config file)
6. Documentation explains telemetry protocol (message types, schemas)
7. Documentation references relevant RFCs for ILP packet format, BTP protocol, addressing
8. Documentation includes sequence diagrams for key workflows (packet forwarding, telemetry emission)
9. Documentation explains design decisions (why Cytoscape.js, why in-memory routing, etc.)
10. Documentation includes section on extending the system (adding new telemetry types, custom visualizations)

---

### Story 4.7: Add Unit and Integration Test Coverage for Dashboard

As a developer,
I want automated tests for dashboard UI components and telemetry integration,
so that I can verify visualization features work correctly.

#### Acceptance Criteria

1. React Testing Library configured for dashboard package
2. Unit tests for key React components: NetworkGraph, PacketAnimation, LogViewer, DetailPanel
3. Unit tests verify component rendering with sample data (mock telemetry)
4. Integration tests verify WebSocket telemetry client correctly processes telemetry messages
5. Integration tests verify graph updates when NODE_STATUS telemetry received
6. Integration tests verify packet animation triggered by PACKET_SENT telemetry
7. Tests mock Cytoscape.js and WebSocket APIs to avoid browser dependencies
8. Dashboard tests run in CI pipeline alongside connector tests
9. Dashboard test coverage >70% (UI tests lower bar than backend logic tests)
10. `npm test` in dashboard package runs all tests and reports coverage

---

### Story 4.8: Create End-to-End Deployment and Routing Test

As a developer,
I want an automated end-to-end test that validates full system operation,
so that I can verify all components work together correctly.

#### Acceptance Criteria

1. E2E test script deploys 3-node network using Docker Compose programmatically
2. Test waits for all containers to report healthy status
3. Test sends ILP Prepare packet from Node A to Node C (requires routing through Node B)
4. Test verifies packet appears in Node A logs (received), Node B logs (forwarded), Node C logs (delivered)
5. Test connects to dashboard telemetry stream and verifies PACKET_SENT events for all hops
6. Test verifies dashboard API or health endpoint shows all 3 nodes connected
7. Test tears down Docker Compose environment after completion
8. Test fails with clear error message if any step fails (container startup, packet routing, telemetry)
9. Test runs in CI pipeline to catch regressions
10. Test documented in README as verification step for contributors

---

### Story 4.9: Performance Testing and Optimization

As a developer,
I want to verify the system meets performance requirements under load,
so that I can ensure NFRs are satisfied.

#### Acceptance Criteria

1. Performance test script sends 100 packets/second through 5-node network
2. Test measures packet forwarding latency (time from ingress to egress)
3. Test measures visualization update latency (time from packet send to dashboard animation)
4. Test verifies NFR1: 5-node network startup completes in <30 seconds
5. Test verifies NFR2: visualization updates within 100ms of packet transmission (95th percentile)
6. Test verifies NFR3: dashboard remains responsive during 100 packets/sec load
7. Test verifies NFR4: no packet loss in logs (100% of sent packets logged)
8. Performance test results documented in `docs/performance.md` with baseline metrics
9. Performance bottlenecks identified and documented (e.g., serialization, network I/O, rendering)
10. Basic optimization applied if critical NFRs not met (e.g., batching telemetry, throttling animation)

---

### Story 4.10: Final Documentation, Examples, and Release Preparation

As a project maintainer,
I want polished documentation, example configurations, and release artifacts,
so that the project is ready for public release and community adoption.

#### Acceptance Criteria

1. All example configurations tested and verified working (linear, mesh, custom)
2. CHANGELOG.md created documenting v0.1.0 MVP features
3. LICENSE file added (MIT or Apache 2.0 for open-source)
4. CONTRIBUTING.md created with guidelines for contributors (code style, testing, PR process)
5. GitHub repository configured with appropriate tags, description, and topics (interledger, ilp, visualization)
6. Pre-built Docker images published to Docker Hub or GitHub Container Registry (optional, but improves UX)
7. README includes badge for build status (CI pipeline), license, and version
8. All TODOs and placeholder comments removed from code
9. Code formatted consistently (Prettier), linted (ESLint), and passes all checks
10. GitHub release created with v0.1.0 tag, release notes, and installation instructions

---

## Checklist Results Report

### Executive Summary

- **Overall PRD Completeness:** 92%
- **MVP Scope Appropriateness:** Just Right
- **Readiness for Architecture Phase:** Ready
- **Most Critical Gaps:**
  1. Missing explicit user journey flows (deferred to UX expert based on brief)
  2. Limited stakeholder input documentation (solo project, expected)
  3. Some integration testing details deferred to implementation

### Category Analysis Table

| Category                         | Status  | Critical Issues                                                |
| -------------------------------- | ------- | -------------------------------------------------------------- |
| 1. Problem Definition & Context  | PASS    | None - backed by comprehensive brief                           |
| 2. MVP Scope Definition          | PASS    | None - clear in/out scope with rationale                       |
| 3. User Experience Requirements  | PARTIAL | User flows not detailed (deferred to UX expert per next steps) |
| 4. Functional Requirements       | PASS    | None - 20 FRs with RFC traceability                            |
| 5. Non-Functional Requirements   | PASS    | None - 12 NFRs with measurable targets                         |
| 6. Epic & Story Structure        | PASS    | None - logical sequencing, appropriate sizing                  |
| 7. Technical Guidance            | PASS    | None - comprehensive tech stack decisions                      |
| 8. Cross-Functional Requirements | PARTIAL | Data schema deferred to architect (appropriate)                |
| 9. Clarity & Communication       | PASS    | None - clear technical writing throughout                      |

### Top Issues by Priority

**BLOCKERS:** None

**HIGH:**

- User journey flow diagrams not included (mitigated: UX expert will handle per next steps section)

**MEDIUM:**

- Stakeholder input section sparse (acceptable for solo open-source project)
- Integration testing details high-level (acceptable at PRD stage)

**LOW:**

- Could add more visual diagrams for network topology examples
- Could expand performance benchmarking details

### MVP Scope Assessment

**Scope is appropriate:**

- Each epic delivers incremental value (protocol → network → visualization → polish)
- Stories sized for AI agent execution (2-4 hour chunks per brief guidance)
- 27 stories across 4 epics = realistic for 3-month timeline with part-time effort
- Out-of-scope items clearly documented (STREAM, settlement engines, production security)

**No features recommended for cutting** - all are essential for "observable ILP network" core value proposition

**No missing essential features identified** - requirements comprehensively cover brief's MVP scope

**Complexity managed:**

- Epic 1 tackles highest risk (RFC implementation) first
- BTP protocol isolated in Epic 2 for focus
- Visualization (Epic 3) builds on stable foundation
- Epic 4 is lower risk (polish/docs)

**Timeline realism:** 3-month estimate reasonable given:

- Monorepo reduces integration overhead
- TypeScript shared types streamline development
- In-memory architecture simplifies state management
- Solo developer can maintain focus without coordination overhead

### Technical Readiness

**Technical constraints clarity:** Excellent

- All tech stack decisions documented with rationale
- RFC compliance requirements explicit (ILPv4, BTP, OER, addressing)
- Docker/containerization approach clear
- Performance targets quantified (NFRs)

**Identified technical risks:**

- BTP WebSocket implementation complexity (mitigated: Epic 2 dedicated to this)
- Visualization performance at high packet rates (mitigated: NFR3 specifies target, Story 4.9 validates)
- Custom ILP packet implementation (mitigated: educational value outweighs risk, test vectors ensure correctness)

**Areas for architect investigation:**

- Telemetry protocol design (push vs pull, batching strategy)
- Cytoscape.js layout algorithm selection for different topologies
- Docker networking configuration for BTP WebSocket communication
- Performance optimization strategies if NFRs not initially met

### Recommendations

**For PM:**

1. ✅ PRD is ready to hand off to UX expert and architect
2. Consider adding simple topology diagram to PRD (optional, low priority)
3. After UX expert completes work, validate that UI flows align with functional requirements

**For UX Expert:**

1. Create detailed user journey flows for primary use cases:
   - Deploy network and observe first packet
   - Debug failed packet routing
   - Experiment with custom topology
2. Design detailed wireframes for dashboard UI (network graph, packet detail panel, log viewer)
3. Validate information architecture supports core user goals (observability, debugging)

**For Architect:**

1. Design telemetry protocol specification (message schemas, WebSocket transport details)
2. Define module boundaries and interfaces between packages (connector, dashboard, shared)
3. Create sequence diagrams for critical flows (packet forwarding with telemetry emission)
4. Specify Docker networking configuration and container orchestration details
5. Design testing strategy (unit, integration, E2E) with concrete examples
6. Investigate and recommend Cytoscape.js layout algorithms for visualization

**Next Actions:**

1. Output full PRD to docs/prd.md
2. Generate UX expert prompt
3. Generate architect prompt
4. (Optional) Create visual diagrams to supplement PRD

### Final Decision

✅ **READY FOR ARCHITECT**

The PRD and epics are comprehensive, properly structured, and provide sufficient detail for architectural design to proceed. The functional and non-functional requirements are clear and testable. The epic breakdown follows agile best practices with logical sequencing and appropriate story sizing. Technical assumptions provide clear constraints for the architect. The minor gaps identified (user flows, data schema details) are appropriately deferred to specialist roles (UX expert, architect) and do not block progress.

---

## Next Steps

### UX Expert Prompt

You are the UX/Design expert for the ILP Connector Visualization project. Please review the attached PRD (docs/prd.md) and Project Brief (docs/brief.md), then create detailed UX specifications including:

1. **User Journey Flows** - Map the three primary user journeys: (a) deploying network and observing first packet, (b) debugging failed packet routing, (c) experimenting with custom topology
2. **Dashboard Wireframes** - Design the visualization dashboard UI including network topology graph, packet animation layer, packet detail panel, node status panel, and log viewer
3. **Information Architecture** - Organize dashboard components for optimal observability and debugging workflows
4. **Interaction Design** - Specify how users interact with network graph (zoom, pan, click packets/nodes), log filtering, and panel management

Your designs should prioritize developer/researcher workflows and technical precision over aesthetic polish. Reference the UI Design Goals section in the PRD for constraints and direction.

### Architect Prompt

You are the Technical Architect for the ILP Connector Visualization project. Please review the attached PRD (docs/prd.md) and Project Brief (docs/brief.md), then create the technical architecture specification including:

1. **System Architecture** - Design the component architecture (connector nodes, dashboard, telemetry aggregation) with clear module boundaries and interfaces
2. **Data Flow Diagrams** - Document packet flow, BTP communication, and telemetry emission with sequence diagrams
3. **Telemetry Protocol Specification** - Define telemetry message schemas, WebSocket transport, and aggregation strategy
4. **Docker Architecture** - Specify container networking, Compose orchestration, and configuration management
5. **Testing Strategy** - Design unit, integration, and E2E testing approach with concrete examples
6. **Technology Selection Details** - Validate and refine tech stack decisions from PRD Technical Assumptions section

Follow the functional requirements (FR1-FR20), non-functional requirements (NFR1-NFR12), and technical assumptions from the PRD. Ensure all architectural decisions support the core goal: observable ILP network with real-time visualization.
