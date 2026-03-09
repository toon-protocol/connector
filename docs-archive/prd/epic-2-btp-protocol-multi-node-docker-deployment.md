# Epic 2: BTP Protocol & Multi-Node Docker Deployment

**Goal:** Implement the Bilateral Transfer Protocol (RFC-0023) to enable WebSocket-based communication between ILP connector nodes, containerize the connector application with Docker, and create Docker Compose orchestration that deploys configurable multi-node networks. This epic delivers a fully functional N-node ILP network running in containers with BTP connections established between peers.

## Story 2.1: Implement BTP WebSocket Server

As a connector node,
I want to accept incoming BTP connections from peer connectors via WebSocket,
so that I can receive ILP packets from upstream peers.

### Acceptance Criteria

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

## Story 2.2: Implement BTP WebSocket Client

As a connector node,
I want to initiate BTP connections to peer connectors and send ILP packets,
so that I can forward packets to downstream peers.

### Acceptance Criteria

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

## Story 2.3: Integrate BTP with Packet Forwarding

As a connector,
I want the packet handler to use BTP clients to send forwarded packets to next-hop peers,
so that packets can traverse multiple connector hops.

### Acceptance Criteria

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

## Story 2.4: Create Dockerfile for Connector Application

As a DevOps engineer,
I want a Dockerfile that builds and packages the connector application,
so that I can deploy connector nodes as containers.

### Acceptance Criteria

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

## Story 2.5: Create Docker Compose Configuration for Multi-Node Network

As a developer,
I want a Docker Compose file that deploys N interconnected connector nodes,
so that I can run a multi-node ILP network with one command.

### Acceptance Criteria

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

## Story 2.6: Implement Configuration File Loading for Topology

As a connector operator,
I want to define routing tables and peer connections in a YAML configuration file,
so that I can easily specify network topology without modifying code.

### Acceptance Criteria

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

## Story 2.7: Add Health Checks and Container Monitoring

As a DevOps engineer,
I want health check endpoints and container health status reporting,
so that I can verify connectors are operational after deployment.

### Acceptance Criteria

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
