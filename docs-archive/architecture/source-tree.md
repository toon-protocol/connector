# Source Tree

```
connector/                              # Monorepo root
├── packages/
│   ├── connector/                      # ILP Connector library and CLI
│   │   ├── src/
│   │   │   ├── core/
│   │   │   │   ├── connector-node.ts           # Main ConnectorNode orchestrator
│   │   │   │   ├── packet-handler.ts           # ILP packet processing logic
│   │   │   │   ├── payment-handler.ts          # Payment handling logic
│   │   │   │   └── local-delivery-client.ts    # Local packet delivery
│   │   │   ├── btp/
│   │   │   │   ├── btp-server.ts               # BTP WebSocket server
│   │   │   │   ├── btp-client.ts               # BTP WebSocket client
│   │   │   │   ├── btp-client-manager.ts       # Peer connection manager
│   │   │   │   ├── btp-message-parser.ts       # BTP protocol encoding/decoding
│   │   │   │   ├── btp-types.ts                # BTP message types and type guards
│   │   │   │   └── btp-claim-types.ts          # Claim protocol types (BaseClaimMessage, EVMClaimMessage)
│   │   │   ├── routing/
│   │   │   │   ├── routing-table.ts            # Routing table implementation
│   │   │   │   └── route-lookup.ts             # Longest-prefix matching
│   │   │   ├── settlement/
│   │   │   │   ├── unified-settlement-executor.ts      # EVM settlement executor (routes SETTLEMENT_REQUIRED)
│   │   │   │   ├── settlement-executor.ts              # Base settlement executor
│   │   │   │   ├── settlement-coordinator.ts           # Settlement coordination logic
│   │   │   │   ├── settlement-monitor.ts               # Balance threshold monitoring
│   │   │   │   ├── settlement-api.ts                   # Settlement REST API endpoints
│   │   │   │   ├── channel-manager.ts                  # Payment channel lifecycle management
│   │   │   │   ├── payment-channel-sdk.ts              # EVM on-chain operations (ethers.js)
│   │   │   │   ├── claim-sender.ts                     # Send claims via BTP (Epic 17)
│   │   │   │   ├── claim-sender-db-schema.ts           # SQLite schema for sent claims
│   │   │   │   ├── claim-receiver.ts                   # Receive/verify claims via BTP (Epic 17)
│   │   │   │   ├── claim-receiver-db-schema.ts         # SQLite schema for received claims
│   │   │   │   ├── claim-redemption-service.ts         # On-chain claim redemption
│   │   │   │   ├── eip712-helper.ts                    # EIP-712 domain separator and types
│   │   │   │   ├── account-manager.ts                  # Double-entry TigerBeetle accounting
│   │   │   │   ├── account-id-generator.ts             # Deterministic account ID generation
│   │   │   │   ├── account-metadata.ts                 # TigerBeetle user_data encoding
│   │   │   │   ├── metrics-collector.ts                # Settlement metrics collection
│   │   │   │   ├── ledger-client.ts                    # ILedgerClient interface
│   │   │   │   ├── in-memory-ledger-client.ts          # In-memory ledger (dev/testing)
│   │   │   │   ├── tigerbeetle-client.ts               # TigerBeetle client adapter
│   │   │   │   ├── tigerbeetle-batch-writer.ts         # High-throughput batch operations
│   │   │   │   ├── tigerbeetle-errors.ts               # TigerBeetle error types
│   │   │   │   └── types.ts                            # Settlement types (PeerConfig, enums)
│   │   │   ├── wallet/
│   │   │   │   ├── agent-wallet.ts             # Agent wallet implementation
│   │   │   │   ├── wallet-db-schema.ts         # Wallet database schema
│   │   │   │   └── balance-tracker.ts          # Balance tracking
│   │   │   ├── telemetry/
│   │   │   │   ├── telemetry-emitter.ts        # Telemetry event emission
│   │   │   │   ├── telemetry-buffer.ts         # Event buffering for high throughput
│   │   │   │   └── types.ts                    # Telemetry message types
│   │   │   ├── explorer/
│   │   │   │   ├── event-store.ts              # libSQL telemetry event storage
│   │   │   │   ├── event-store.test.ts         # EventStore unit tests
│   │   │   │   └── index.ts                    # Explorer module exports
│   │   │   ├── security/
│   │   │   │   ├── ip-allowlist.ts             # IP-based access control
│   │   │   │   ├── deployment-mode.ts          # Deployment mode restrictions
│   │   │   │   ├── input-validator.ts          # Input validation
│   │   │   │   └── auth-middleware.ts          # Authentication middleware
│   │   │   ├── observability/
│   │   │   │   ├── health-check.ts             # Health check implementation
│   │   │   │   ├── metrics.ts                  # Metrics collection
│   │   │   │   └── structured-logger.ts        # Pino logger wrapper
│   │   │   ├── discovery/
│   │   │   │   ├── peer-discovery.ts           # Peer discovery logic
│   │   │   │   └── service-registry.ts         # Service registration
│   │   │   ├── config/
│   │   │   │   ├── config-loader.ts            # YAML/object config loading
│   │   │   │   ├── types.ts                    # Configuration type definitions
│   │   │   │   ├── topology-validator.ts       # Topology validation
│   │   │   │   ├── environment-validator.ts    # Environment variable validation
│   │   │   │   ├── key-manager-config.ts       # Key manager configuration
│   │   │   │   └── index.ts                    # Config module exports
│   │   │   ├── http/
│   │   │   │   ├── health-server.ts            # Express health check endpoint
│   │   │   │   ├── explorer-server.ts          # Explorer UI HTTP server
│   │   │   │   └── api-routes.ts               # API route handlers
│   │   │   ├── cli/
│   │   │   │   ├── index.ts                    # CLI entry point
│   │   │   │   ├── setup-command.ts            # Interactive setup
│   │   │   │   ├── health-command.ts           # Health check command
│   │   │   │   └── validate-command.ts         # Config validation command
│   │   │   ├── encoding/
│   │   │   │   ├── oer-codec.ts                # OER encoder/decoder
│   │   │   │   └── packet-serializer.ts        # Packet serialization
│   │   │   ├── database/
│   │   │   │   └── libsql-client.ts            # libSQL database client
│   │   │   ├── facilitator/
│   │   │   │   └── payment-facilitator.ts      # Payment facilitation logic
│   │   │   ├── performance/
│   │   │   │   ├── benchmark.ts                # Performance benchmarking
│   │   │   │   └── load-test.ts                # Load testing utilities
│   │   │   ├── test/
│   │   │   │   └── integration/                # Integration tests
│   │   │   ├── test-utils/
│   │   │   │   ├── mocks.ts                    # Testing mocks
│   │   │   │   └── fixtures.ts                 # Test fixtures
│   │   │   ├── utils/
│   │   │   │   └── logger.ts                   # Pino logger configuration
│   │   │   ├── index.ts                        # Package public API
│   │   │   ├── lib.ts                          # Library exports
│   │   │   └── main.ts                         # Main entry point
│   │   ├── explorer-ui/                        # Built-in Explorer UI (React)
│   │   │   ├── src/
│   │   │   │   ├── App.tsx                     # Main application component
│   │   │   │   ├── main.tsx                    # React entry point
│   │   │   │   ├── index.css                   # Tailwind + shadcn theme
│   │   │   │   ├── components/
│   │   │   │   │   ├── EventTable.tsx          # Event streaming table
│   │   │   │   │   ├── Header.tsx              # Header with node ID
│   │   │   │   │   └── ui/                     # shadcn/ui components
│   │   │   │   ├── hooks/
│   │   │   │   │   ├── useEventStream.ts       # WebSocket connection hook
│   │   │   │   │   └── useEventStream.test.ts
│   │   │   │   └── lib/
│   │   │   │       ├── event-types.ts          # Frontend telemetry types
│   │   │   │       └── utils.ts                # shadcn cn() helper
│   │   │   ├── index.html
│   │   │   ├── vite.config.ts
│   │   │   ├── tailwind.config.js
│   │   │   └── package.json
│   │   ├── test/
│   │   │   ├── unit/
│   │   │   │   ├── packet-handler.test.ts
│   │   │   │   ├── routing-table.test.ts
│   │   │   │   └── btp-message-parser.test.ts
│   │   │   └── integration/
│   │   │       ├── multi-node-forwarding.test.ts
│   │   │       └── telemetry-event-store.test.ts
│   │   ├── Dockerfile                          # Connector container build
│   │   ├── package.json
│   │   └── tsconfig.json
│   │
│   ├── shared/                                 # Shared TypeScript types and utilities
│   │   ├── src/
│   │   │   ├── types/
│   │   │   │   ├── ilp.ts                      # ILP packet type definitions
│   │   │   │   ├── btp.ts                      # BTP message types
│   │   │   │   ├── routing.ts                  # Routing table types
│   │   │   │   ├── telemetry.ts                # Telemetry event types
│   │   │   │   └── payment-channel-telemetry.ts # Payment channel telemetry types
│   │   │   ├── encoding/
│   │   │   │   └── oer.ts                      # OER encoder/decoder implementation
│   │   │   ├── validation/
│   │   │   │   └── ilp-address.ts              # ILP address validation (RFC-0015)
│   │   │   └── index.ts                        # Shared package exports
│   │   ├── test/
│   │   │   ├── encoding/
│   │   │   │   └── oer.test.ts                 # OER encoding test vectors
│   │   │   └── validation/
│   │   │       └── ilp-address.test.ts
│   │   ├── package.json
│   │   └── tsconfig.json
│   │
│   ├── contracts/                              # Ethereum smart contracts
│   │   ├── contracts/
│   │   │   ├── AGENT.sol                       # ERC20 token contract
│   │   │   ├── TokenNetwork.sol                # Payment channel network
│   │   │   ├── TokenNetworkRegistry.sol        # TokenNetwork factory
│   │   │   └── libraries/
│   │   │       └── ChannelManagerLibrary.sol   # Channel state validation
│   │   ├── scripts/
│   │   │   ├── deploy.ts                       # Deployment scripts
│   │   │   └── deploy-local.ts                 # Local Anvil deployment
│   │   ├── test/
│   │   │   ├── TokenNetwork.test.ts
│   │   │   └── TokenNetworkRegistry.test.ts
│   │   ├── hardhat.config.ts
│   │   ├── package.json
│   │   └── tsconfig.json
│   │
│   └── dashboard/                              # Legacy visualization dashboard (deferred)
│       ├── src/
│       │   ├── backend/
│       │   │   ├── telemetry-server.ts
│       │   │   └── http-server.ts
│       │   └── frontend/
│       │       └── (legacy React components)
│       └── package.json
│
├── tools/                                      # CLI utilities
│   ├── send-packet/
│   │   ├── src/
│   │   │   ├── index.ts                        # Test packet sender CLI
│   │   │   ├── btp-sender.ts                   # BTP client wrapper
│   │   │   └── packet-factory.ts               # Packet creation helpers
│   │   ├── package.json
│   │   └── tsconfig.json
│   │
│   └── fund-peers/
│       ├── src/
│       │   └── index.ts                        # Peer funding utility
│       ├── package.json
│       └── tsconfig.json
│
├── examples/                                   # Pre-configured topology YAML files
│   ├── linear-3-nodes-{a,b,c}.yaml            # 3-node linear chain
│   ├── linear-5-nodes-{a,b,c,d,e}.yaml        # 5-node linear chain
│   ├── mesh-4-nodes-{a,b,c,d}.yaml            # 4-node full mesh
│   ├── hub-spoke-{hub,spoke1,spoke2,spoke3}.yaml # Hub-and-spoke topology
│   ├── complex-8-node/                         # 8-node complex network
│   │   ├── hub-1.yaml
│   │   ├── hub-2.yaml
│   │   └── spoke-{1a,1b,1c,2a,2b,2c}.yaml
│   ├── multihop-peer{1..5}.yaml               # 5-peer multihop configs
│   ├── production-node-{1,2,3}.yaml           # Production configs
│   ├── production-single-node.yaml
│   ├── agent-runtime-connector.yaml            # Agent runtime config
│   └── test-*.yaml                             # Test configurations
│
├── docker-compose files (15+ topologies):
│   ├── docker-compose.yml                      # Default 3-node linear
│   ├── docker-compose-5-node.yml              # 5-node linear
│   ├── docker-compose-5-peer-multihop.yml     # 5-peer with TigerBeetle
│   ├── docker-compose-5-peer-agent-runtime.yml # Agent runtime + BLS
│   ├── docker-compose-5-peer-nostr-spsp.yml   # Agent society + Nostr
│   ├── docker-compose-unified.yml              # Full 3-layer stack (16 services)
│   ├── docker-compose-agent-runtime.yml        # Agent runtime deployment
│   ├── docker-compose-mesh.yml                 # 4-node mesh
│   ├── docker-compose-hub-spoke.yml            # Hub-and-spoke
│   ├── docker-compose-complex.yml              # 8-node complex
│   ├── docker-compose-dev.yml                  # Dev infrastructure only
│   ├── docker-compose-staging.yml              # Staging environment
│   ├── docker-compose-production.yml           # Production template
│   ├── docker-compose-production-3node.yml     # Production 3-node cluster
│   └── docker-compose-monitoring.yml           # Production with monitoring
│
├── docker/                                     # Docker Compose templates
│   ├── docker-compose.linear.yml               # Linear topology template
│   ├── docker-compose.mesh.yml                 # Mesh topology template
│   ├── docker-compose.hub-spoke.yml            # Hub-spoke template
│   └── docker-compose.custom-template.yml      # Custom topology template
│
├── docs/                                       # Documentation
│   ├── architecture.md                         # Main architecture document
│   ├── architecture/                           # Sharded architecture sections
│   │   ├── index.md                            # Architecture table of contents
│   │   ├── introduction.md
│   │   ├── high-level-architecture.md
│   │   ├── tech-stack.md
│   │   ├── data-models.md
│   │   ├── components.md
│   │   ├── external-apis.md
│   │   ├── core-workflows.md
│   │   ├── database-schema.md
│   │   ├── source-tree.md                      # This file
│   │   ├── infrastructure-and-deployment.md
│   │   ├── error-handling-strategy.md
│   │   ├── coding-standards.md
│   │   ├── test-strategy-and-standards.md
│   │   ├── security.md
│   │   ├── agent-society-protocol.md
│   │   ├── routing-configuration.md
│   │   ├── payment-channel-at-connection-design.md
│   │   ├── ccp-protocol-explanation.md
│   │   └── next-steps.md
│   ├── prd.md                                  # Product requirements document
│   ├── brief.md                                # Project brief
│   └── rfcs/                                   # Copied relevant Interledger RFCs
│       ├── rfc-0027-ilpv4.md
│       ├── rfc-0023-btp.md
│       └── rfc-0030-oer.md
│
├── .github/
│   └── workflows/
│       ├── ci.yml                              # GitHub Actions CI pipeline
│       └── docker-build.yml                    # Docker image build workflow
│
├── scripts/
│   ├── install-tigerbeetle-macos.sh           # TigerBeetle installation
│   ├── start-tigerbeetle-dev.sh               # Start TigerBeetle dev server
│   ├── stop-tigerbeetle-dev.sh                # Stop TigerBeetle dev server
│   └── validate-packages.mjs                   # Package validation
│
├── package.json                                # Root package.json (workspaces)
├── tsconfig.base.json                          # Shared TypeScript configuration
├── .eslintrc.json                              # ESLint configuration
├── .prettierrc.json                            # Prettier configuration
├── .gitignore
├── README.md                                   # Project overview and quick start
├── CONTRIBUTING.md                             # Contribution guidelines
├── LICENSE                                     # MIT license
└── CHANGELOG.md                                # Version history
```

## Key Directory Decisions

1. **Monorepo with npm workspaces:** Simplifies dependency management and type sharing
2. **Clear package boundaries:** `connector`, `shared`, `contracts` are independently buildable and publishable
3. **EVM settlement:** Ethereum (Solidity) smart contracts for payment channels
4. **Built-in Explorer UI:** Embedded within connector package at `explorer-ui/`, served by connector HTTP server
5. **Co-located tests:** Test files alongside source (`*.test.ts` next to `*.ts`) for better discoverability
6. **Comprehensive module organization:** 20 specialized modules in connector (core, btp, routing, settlement, wallet, security, observability, etc.)
7. **Docker configs at root:** 15+ topology configurations for easy access via `docker-compose up`
8. **Examples directory:** 29+ pre-configured topology YAML files for quick experimentation
9. **Tools separate:** CLI utilities (`send-packet`, `fund-peers`) independent of main packages
10. **CLI binary:** Connector package includes `connector` CLI for setup, validation, and health checks

## Notes

- **Dashboard package:** Legacy package retained for reference, replaced by `explorer-ui/` embedded in connector
- **Security modules:** Comprehensive security features (IP allowlisting, deployment mode restrictions, input validation)
- **Observability-first:** Dedicated modules for telemetry, health checks, metrics, and structured logging
- **Flexible deployment:** Library (programmatic), CLI (standalone), or Docker (orchestrated)
- **Production-ready:** Includes staging/production compose files, monitoring stack, and multi-environment support
