# Technical Assumptions

## Repository Structure: Monorepo

The project will use a **monorepo** structure managed with npm workspaces (or similar tooling) containing:

- `packages/connector` - ILP connector implementation with BTP plugin
- `packages/dashboard` - React-based visualization UI
- `packages/shared` - Shared TypeScript types, utilities, and ILP packet definitions
- `docker/` - Docker Compose configurations and Dockerfiles
- `examples/` - Sample topology configurations

**Rationale:** Monorepo simplifies dependency management, enables code sharing (especially TypeScript types between connector and dashboard), and streamlines the development workflow for a single developer. The brief explicitly suggests this structure in the Technical Considerations section.

## Service Architecture

**Microservices architecture within Docker containers:**

- **Connector nodes:** Multiple identical containers (one per ILP connector), each running independently
- **Dashboard service:** Single container serving the React UI and WebSocket server for telemetry aggregation
- **No shared database:** Each connector maintains in-memory state (routing tables, peer connections)
- **Communication:**
  - BTP connections between connectors (WebSocket)
  - Telemetry from connectors to dashboard (WebSocket or HTTP POST)
  - Dashboard serves UI to user's browser (HTTP)

**Rationale:** Aligns with brief's Docker-based deployment model and observability requirements. Microservices architecture allows independent scaling of connector nodes and matches the multi-node network simulation goal. In-memory state sufficient for MVP (no persistence requirement per brief).

## Testing Requirements

**Unit + Integration testing with manual testing convenience methods:**

- **Unit tests:** Jest for core ILP packet handling, routing logic, BTP message parsing (target 80% coverage per NFR8)
- **Integration tests:** Test multi-connector packet forwarding scenarios using Docker Compose test configurations
- **Manual testing utilities:** CLI tools for sending test packets, inspecting routing tables, and triggering specific scenarios
- **No E2E UI testing:** Dashboard UI verified manually (E2E test infrastructure deferred to post-MVP)
- **Docker health checks:** Built-in container health verification (FR16)

**Rationale:** Balances quality assurance with MVP timeline constraints. Unit tests protect core protocol logic (highest risk area). Integration tests validate multi-node scenarios (key differentiator). Manual testing tools support the educational use case (developers experimenting with network). Full E2E automation deferred given solo developer resource constraint.

## Additional Technical Assumptions and Requests

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
