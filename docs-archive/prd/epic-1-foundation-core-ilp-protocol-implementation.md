# Epic 1: Foundation & Core ILP Protocol Implementation

**Goal:** Establish the foundational monorepo project structure with Git, CI/CD, and development tooling, while implementing the core ILPv4 packet encoding/decoding and routing logic. This epic delivers a working single-node ILP packet processor that can parse, validate, and route packets according to RFC-0027, with comprehensive unit tests and structured logging infrastructure in place.

## Story 1.1: Initialize Monorepo with TypeScript & Development Tooling

As a developer,
I want a well-structured monorepo with TypeScript configuration, linting, and testing framework,
so that I have a solid foundation for building the ILP connector and dashboard packages.

### Acceptance Criteria

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

## Story 1.2: Implement ILP Packet Type Definitions (TypeScript Interfaces)

As a connector developer,
I want TypeScript type definitions for all ILPv4 packet types and address formats,
so that I have type-safe representations of ILP protocol data structures.

### Acceptance Criteria

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

## Story 1.3: Implement OER Encoding/Decoding for ILP Packets

As a connector developer,
I want functions to encode ILP packets to binary format and decode binary data to packet objects,
so that I can serialize packets for transmission according to RFC-0030.

### Acceptance Criteria

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

## Story 1.4: Implement In-Memory Routing Table

As a connector operator,
I want a routing table that stores destination prefixes and next-hop peer mappings,
so that the connector can determine where to forward packets.

### Acceptance Criteria

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

## Story 1.5: Implement Core Packet Forwarding Logic

As an ILP connector,
I want to receive ILP Prepare packets, look up routes, and forward to the appropriate next-hop peer,
so that I can route payments through the network.

### Acceptance Criteria

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

## Story 1.6: Integrate Pino Structured Logging

As a connector operator,
I want all ILP operations logged in structured JSON format with appropriate log levels,
so that I can debug issues and monitor connector behavior.

### Acceptance Criteria

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

## Story 1.7: Add Unit Tests for ILP Core Logic

As a developer,
I want comprehensive unit tests for packet encoding, routing, and forwarding logic,
so that I can verify RFC compliance and prevent regressions.

### Acceptance Criteria

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
