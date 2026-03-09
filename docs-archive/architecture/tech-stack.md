# Tech Stack

**CRITICAL SECTION - DEFINITIVE TECHNOLOGY CHOICES**

This section represents the single source of truth for all technology decisions. All implementation must reference these exact versions and choices.

## Cloud Infrastructure

- **Provider:** None (Local Docker deployment for MVP)
- **Key Services:** Docker Engine, Docker Compose
- **Deployment Regions:** Localhost only (future: cloud-agnostic Kubernetes)

## Technology Stack Table

| Category                       | Technology                                          | Version          | Purpose                                               | Rationale                                                                                                                                                |
| ------------------------------ | --------------------------------------------------- | ---------------- | ----------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Language**                   | TypeScript                                          | 5.3.3            | Primary development language for all packages         | Strong typing ensures RFC compliance, excellent IDE support, enables type sharing across packages, aligns with Interledger.js ecosystem                  |
| **Runtime**                    | Node.js                                             | ≥22.11.0 LTS     | JavaScript runtime for connector and backend services | LTS version guarantees stability (as of October 2024), wide ecosystem, asynchronous I/O ideal for WebSocket handling, Docker images readily available    |
| **Package Manager**            | npm                                                 | ≥10.0.0          | Dependency management and workspace orchestration     | Built-in workspaces feature supports monorepo, standard tooling, no additional setup required                                                            |
| **Backend Framework**          | Express.js (optional)                               | 4.18.x           | HTTP server for health endpoint and Explorer UI       | Lightweight, well-documented, sufficient for API and static file serving, peer dependency (optional)                                                     |
| **WebSocket Library (Server)** | ws                                                  | ^8.16.0          | WebSocket server for BTP connections                  | Lightweight, standard Node.js WebSocket library, RFC 6455 compliant, widely used                                                                         |
| **Logging Library**            | Pino                                                | ^8.21.0          | Structured JSON logging                               | High-performance (minimal overhead), excellent TypeScript support, structured JSON output, child logger support for correlation IDs                      |
| **Testing Framework**          | Jest                                                | 29.7.x           | Unit and integration testing                          | Industry standard, excellent TypeScript support, snapshot testing, mocking capabilities, coverage reporting                                              |
| **Linting**                    | ESLint                                              | 8.56.x           | Code quality and consistency                          | Enforce coding standards, catch common errors, TypeScript integration via @typescript-eslint                                                             |
| **Code Formatting**            | Prettier                                            | 3.2.x            | Automated code formatting                             | Consistent code style, integrates with ESLint, reduces style debates                                                                                     |
| **ILP Packet Encoding**        | Custom OER Implementation                           | N/A              | Encode/decode ILP packets per RFC-0030                | Educational value of building from scratch, no suitable existing library with TypeScript types, enables deep RFC understanding                           |
| **Configuration Format**       | YAML + dotenv                                       | js-yaml 4.1.x    | Topology definitions (YAML), runtime config (ENV)     | YAML human-readable for topology files, ENV vars integrate with Docker Compose, standard conventions                                                     |
| **Container Base Image**       | node:22-alpine                                      | 22-alpine        | Docker base image for all containers                  | Small footprint (~150MB), official Node.js image, Alpine Linux security benefits, faster startup                                                         |
| **Container Orchestration**    | Docker Compose                                      | 2.24.x           | Multi-node network deployment                         | Simple declarative configuration, standard developer tool, supports health checks and networking                                                         |
| **Version Control**            | Git                                                 | 2.x              | Source control with conventional commits              | Industry standard, conventional commits enable changelog automation                                                                                      |
| **CI/CD**                      | GitHub Actions                                      | N/A              | Automated testing, linting, and Docker builds         | Free for open-source, GitHub integration, supports matrix testing across Node versions                                                                   |
| **Database (Accounting)**      | In-memory ledger (default) / TigerBeetle (optional) | N/A / 0.16.68    | Balance tracking for peer settlement                  | In-memory Map-based ledger is the zero-dependency default; TigerBeetle available as optional peer dependency for high-performance production deployments |
| **Database (Wallet)**          | better-sqlite3 (optional)                           | ^11.8.1          | Agent wallet state and payment channel tracking       | Embedded SQLite database for wallet persistence, optional peer dependency                                                                                |
| **Database (Explorer)**        | libSQL (optional)                                   | 0.14.0           | Telemetry event storage for Explorer UI               | SQLite fork with MVCC concurrent writes, async API, optional for Explorer event persistence                                                              |
| **Blockchain - Ethereum**      | ethers                                              | ^6.16.0          | Ethereum smart contract interactions                  | Standard Ethereum library v6, TokenNetwork payment channels, ERC20 operations                                                                            |
| **AI SDK**                     | ai (Vercel AI SDK)                                  | ^4.3.19          | AI-native event handling with tool calling            | Provider-agnostic model abstraction, built-in tool system, streaming support (optional dependency)                                                       |
| **AI Providers**               | @ai-sdk/anthropic, @ai-sdk/openai                   | ^1.2.12, ^1.3.24 | Anthropic and OpenAI model provider adapters          | Pluggable provider system for AI model integration (optional dependencies)                                                                               |
| **Schema Validation**          | Zod                                                 | ^3.25.76         | Runtime schema validation and type inference          | TypeScript-first schema validation, AI SDK parameter validation                                                                                          |
| **Explorer UI**                | React + Vite + shadcn-ui                            | 18.x / 5.x / v4  | Built-in packet/event visualization UI                | Modern frontend stack embedded in connector package, served via Express                                                                                  |

**Important Notes:**

1. **External APIs Required:** EVM-compatible blockchains (Ethereum, Base, Sepolia, etc.)
2. **Monorepo Package Structure:**
   - `packages/connector` - Production ILP connector (@crosstown/connector npm package)
   - `packages/connector/explorer-ui` - Built-in React visualization UI (embedded)
   - `packages/shared` - Shared TypeScript types and utilities (@crosstown/shared npm package)
   - `packages/contracts` - Ethereum Solidity smart contracts (TokenNetwork, AGENT ERC20)
   - `packages/dashboard` - Legacy dashboard (deferred, superseded by explorer-ui)
3. **Published Packages:**
   - `@crosstown/connector` - Main connector library and CLI
   - `@crosstown/shared` - Shared types and utilities
4. **TypeScript Configuration:** Strict mode enabled across all packages, shared tsconfig.base.json in monorepo root
5. **Version Pinning Strategy:** Patch versions flexible (^), minor versions locked for stability, LTS/stable releases preferred
6. **Dependency Strategy:** Core dependencies required, peer dependencies optional (Express, TigerBeetle, better-sqlite3), AI features optional
7. **License Compatibility:** All dependencies MIT or Apache 2.0 compatible (open-source project)

**EVM-Only Settlement:** Production connector supports EVM blockchain settlement via Ethereum TokenNetwork contracts and ERC20 operations.

**Zero-Config Default:** No external databases or services required - in-memory ledger and file-based persistence work out of the box.
