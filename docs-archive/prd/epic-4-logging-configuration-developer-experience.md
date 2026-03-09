# Epic 4: Logging, Configuration & Developer Experience

**Goal:** Complete the developer-facing features that make the tool production-ready: comprehensive structured logging with a filterable log viewer UI, support for multiple pre-configured network topologies, a test packet sender utility for experimentation, and complete documentation. This epic transforms the working system into a polished, user-friendly development tool.

## Story 4.1: Implement Filterable Log Viewer in Dashboard

As a user,
I want to view aggregated logs from all connector nodes with filtering and search capabilities,
so that I can debug specific issues without parsing raw log files.

### Acceptance Criteria

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

## Story 4.2: Add Full Mesh Topology Configuration

As a developer,
I want a pre-configured full mesh network topology example,
so that I can experiment with more complex routing scenarios.

### Acceptance Criteria

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

## Story 4.3: Add Custom Topology Configuration Support

As a developer,
I want to define my own custom network topologies via configuration file,
so that I can test specific routing scenarios relevant to my use case.

### Acceptance Criteria

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

## Story 4.4: Create Test Packet Sender Utility

As a developer,
I want a CLI tool to inject test ILP packets into the network,
so that I can observe packet routing without external dependencies.

### Acceptance Criteria

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

## Story 4.5: Add Comprehensive README Documentation

As a new user,
I want clear documentation explaining how to set up and use the ILP connector network,
so that I can get started quickly without prior knowledge of the codebase.

### Acceptance Criteria

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

## Story 4.6: Add Architecture Documentation

As a developer or contributor,
I want architecture documentation explaining how the system components interact,
so that I can understand the codebase and contribute effectively.

### Acceptance Criteria

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

## Story 4.7: Add Unit and Integration Test Coverage for Dashboard

As a developer,
I want automated tests for dashboard UI components and telemetry integration,
so that I can verify visualization features work correctly.

### Acceptance Criteria

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

## Story 4.8: Create End-to-End Deployment and Routing Test

As a developer,
I want an automated end-to-end test that validates full system operation,
so that I can verify all components work together correctly.

### Acceptance Criteria

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

## Story 4.9: Performance Testing and Optimization

As a developer,
I want to verify the system meets performance requirements under load,
so that I can ensure NFRs are satisfied.

### Acceptance Criteria

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

## Story 4.10: Final Documentation, Examples, and Release Preparation

As a project maintainer,
I want polished documentation, example configurations, and release artifacts,
so that the project is ready for public release and community adoption.

### Acceptance Criteria

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
