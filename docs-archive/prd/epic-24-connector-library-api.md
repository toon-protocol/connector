# Epic 24: Connector Library API — Brownfield Enhancement

**Epic Number:** 24
**Priority:** High — Required for ElizaOS in-process integration
**Type:** Library Refactoring
**Dependencies:** Epic 20 (bidirectional middleware), Epic 21 (payment channel admin APIs)

## Epic Goal

Refactor `ConnectorNode` to accept a config object (not a file path), expose `sendPacket()` as a public method, add a `setLocalDeliveryHandler()` hook for direct in-process packet delivery, and surface admin operations as callable methods — enabling `@crosstown/connector` to run embedded inside an ElizaOS Service without HTTP between components.

## Epic Description

### Existing System Context

**Current Functionality:**

- `ConnectorNode` constructor takes `(configFilePath: string, logger: Logger)` and uses `ConfigLoader.loadConfig(configFilePath)` internally to parse YAML
- Local packet delivery uses `LocalDeliveryClient` which POSTs to an external HTTP URL (`/ilp/packets`) — requires a separate agent-runtime process
- Sending packets is done via `BTPClientManager.sendToPeer()` internally, but `ConnectorNode` does not expose a public `sendPacket()` method — external callers must go through `POST /ilp/send` on the admin HTTP API
- Peer registration, route management, balance queries, and channel operations are only accessible via `AdminServer` HTTP endpoints (Epics 20-21)
- Admin operations dispatch through `AdminApi` router → internal services (BTPClientManager, RoutingTable, AccountManager, SettlementMonitor)

**Technology Stack:**

- TypeScript, Express (AdminServer), WebSocket/BTP (peer connections), TigerBeetle (accounting), pino (logging)
- Config: YAML files parsed by `ConfigLoader` with manual validation (throws `ConfigurationError`)

**Integration Points:**

- `packages/connector/src/core/connector-node.ts` — Constructor, lifecycle, new public API surface
- `packages/connector/src/core/local-delivery-client.ts` — HTTP-based local delivery (add function handler alternative)
- `packages/connector/src/core/packet-handler.ts` — Uses LocalDeliveryClient for local packets
- `packages/connector/src/http/admin-api.ts` — HTTP wrappers for admin operations
- `packages/connector/src/btp/btp-client-manager.ts` — `sendToPeer()` for packet sending
- `packages/connector/src/config/types.ts` — `ConnectorConfig` type definition

### Enhancement Details

**What's Being Added:**

1. **Config object constructor** — `ConnectorNode` accepts a `ConnectorConfig` object directly. YAML loading moves to the CLI entrypoint. The validated `ConnectorConfig` type remains the contract.
2. **`setLocalDeliveryHandler(handler)`** — Allows a direct function handler for local packet delivery (replaces HTTP round-trip). `LocalDeliveryClient` HTTP path preserved as fallback when no handler is set.
3. **`sendPacket()` public method** — Exposes packet sending on `ConnectorNode` using existing `PacketHandler` routing logic. This is what agent-society's `BootstrapService`, `NostrSpspClient`, and PAY action will call directly.
4. **Admin operations as methods** — `registerPeer()`, `removePeer()`, `listPeers()`, `getBalance()`, `listRoutes()`, `addRoute()` exposed as methods on `ConnectorNode`. The `AdminServer` HTTP wrapper remains optional for debugging/external tooling.

**How It Integrates:**

- agent-society imports `ConnectorNode`, passes a config object, sets a local delivery handler pointing to `bls.handlePayment()`, and calls `connector.sendPacket()` directly — zero HTTP between components
- The existing HTTP-based paths (`LocalDeliveryClient`, `AdminServer`, `/ilp/send`) continue to work for standalone deployments
- All new methods delegate to the same internal services the HTTP endpoints use — no logic duplication

**Success Criteria:**

1. `ConnectorNode` accepts `ConnectorConfig` object — no file path required
2. `setLocalDeliveryHandler()` enables direct function delivery, bypassing HTTP
3. `sendPacket()` sends ILP Prepare packets and returns Fulfill/Reject
4. Admin operations callable as methods without AdminServer running
5. All existing HTTP-based paths still function (backward compatible)
6. All existing tests pass; new unit tests for each public method
7. TypeScript compilation succeeds across the monorepo

## Stories

### Story 24.1: Accept ConnectorConfig Object in Constructor

**As a** library consumer,
**I want** to instantiate `ConnectorNode` with a config object,
**so that** I can embed the connector in my process without needing a YAML file on disk.

**Scope:**

- **Modify `connector-node.ts` constructor:**
  - Change signature from `(configFilePath: string, logger: Logger)` to `(config: ConnectorConfig | string, logger: Logger)`
  - When `config` is a string, treat it as a file path and load via `ConfigLoader` (backward compatible)
  - When `config` is an object, validate with `ConfigLoader.validateConfig()` directly (skip file loading)
  - Preserve all existing initialization logic after config resolution
- **Modify `ConfigLoader`:**
  - Extract validation logic into a reusable `validateConfig(raw: unknown): ConnectorConfig` method
  - `loadConfig(filePath)` calls `validateConfig` after YAML parsing
  - Constructor uses `validateConfig` when given an object
- **Update tests:**
  - Add unit tests for object-based construction
  - Verify existing file-path tests still pass
  - Test validation errors on invalid config objects

**Acceptance Criteria:**

1. `new ConnectorNode(configObject, logger)` works without a file on disk
2. `new ConnectorNode('/path/to/config.yaml', logger)` still works (backward compatible)
3. Same validation logic applied to both paths — same error messages
4. Invalid config objects produce clear validation errors
5. `ConfigLoader.validateConfig()` exported for external use
6. All existing ConnectorNode tests pass unchanged
7. New tests cover object construction, validation, and error cases

---

### Story 24.2: Add setLocalDeliveryHandler() for Direct In-Process Delivery

**As a** library consumer,
**I want** to register a function handler for local packet delivery,
**so that** inbound ILP packets are delivered directly to my BLS without an HTTP round-trip.

**Scope:**

- **Modify `ConnectorNode`:**
  - Add `setLocalDeliveryHandler(handler: LocalDeliveryHandler): void` method
  - Type: `type LocalDeliveryHandler = (packet: LocalDeliveryRequest, sourcePeerId: string) => Promise<LocalDeliveryResponse>`
  - Must be callable before or after `start()` (handler can be changed at runtime)
- **Modify `PacketHandler` (or `LocalDeliveryClient`):**
  - Add support for a function handler alongside the HTTP client
  - When a function handler is registered, call it directly instead of HTTP POST
  - When no function handler is set, fall back to HTTP client (existing behavior)
  - Error handling: if function handler throws, convert to ILP Reject (same as HTTP failure path)
- **Preserve LocalDeliveryClient:**
  - Keep HTTP client fully functional as fallback
  - No changes to `LocalDeliveryClient` class itself — the handler bypass is upstream
- **Update tests:**
  - Test direct function handler receives correct packet data
  - Test function handler error → ILP Reject conversion
  - Test fallback to HTTP when no handler set
  - Test handler replacement at runtime

**Acceptance Criteria:**

1. `connector.setLocalDeliveryHandler(fn)` registers a direct delivery function
2. Inbound local packets delivered via function handler (no HTTP)
3. Function handler receives `LocalDeliveryRequest` and `sourcePeerId`
4. Function handler errors produce ILP Reject (T00 Internal Error)
5. Without handler set, HTTP `LocalDeliveryClient` is used (backward compatible)
6. Handler can be set/changed before or after `start()`
7. All existing local delivery tests pass unchanged
8. New tests cover handler registration, delivery, errors, and fallback

---

### Story 24.3: Expose sendPacket() as Public Method on ConnectorNode

**As a** library consumer,
**I want** to call `connector.sendPacket()` to send ILP Prepare packets,
**so that** I can initiate payments without going through the HTTP admin API.

**Scope:**

- **Add to `ConnectorNode`:**

  ```typescript
  async sendPacket(params: {
    destination: string;
    amount: bigint;
    executionCondition: Buffer;
    expiresAt: Date;
    data: Buffer;
  }): Promise<ILPFulfillPacket | ILPRejectPacket>
  ```

  - Constructs an ILP Prepare packet from params
  - Routes through `PacketHandler` (uses RoutingTable to find next hop, sends via BTPClientManager)
  - Returns the ILP Fulfill or Reject response
  - Throws if connector not started

- **Reuse existing routing logic:**
  - `PacketHandler.handlePrepare()` or equivalent routing path
  - Same longest-prefix match, same BTPClientManager dispatch
  - Same timeout handling derived from `expiresAt`
- **Update tests:**
  - Test sendPacket routes to correct peer
  - Test sendPacket returns Fulfill on success
  - Test sendPacket returns Reject on routing failure
  - Test sendPacket throws when connector not started

**Acceptance Criteria:**

1. `connector.sendPacket(params)` sends an ILP Prepare and returns Fulfill/Reject
2. Routing uses the same RoutingTable longest-prefix matching as BTP-originated packets
3. Timeout derived from `expiresAt` parameter
4. Returns ILP Reject (not throws) for routing/delivery failures
5. Throws `ConnectorNotStartedError` if called before `start()`
6. `POST /ilp/send` admin endpoint still works (unchanged)
7. New unit tests cover routing, fulfillment, rejection, and error cases

---

### Story 24.4: Expose Admin Operations as Direct Methods

**As a** library consumer,
**I want** to call peer, route, and balance operations directly on `ConnectorNode`,
**so that** I don't need AdminServer HTTP running to manage the connector programmatically.

**Scope:**

- **Add methods to `ConnectorNode`:**
  - `registerPeer(config: PeerRegistrationRequest): Promise<PeerInfo>` — delegates to same logic as `POST /admin/peers`
  - `removePeer(peerId: string): Promise<RemovePeerResult>` — delegates to same logic as `DELETE /admin/peers/:peerId`
  - `listPeers(): PeerInfo[]` — delegates to same logic as `GET /admin/peers`
  - `getBalance(peerId: string): Promise<PeerAccountBalance>` — delegates to same logic as `GET /admin/balances/:peerId`
  - `listRoutes(): RouteInfo[]` — delegates to same logic as `GET /admin/routes`
  - `addRoute(route: RouteInfo): void` — delegates to same logic as `POST /admin/routes`
  - `removeRoute(prefix: string): void` — delegates to same logic as `DELETE /admin/routes/:prefix`
- **Refactor admin logic extraction:**
  - Extract core logic from `AdminApi` route handlers into reusable service methods (or on ConnectorNode directly)
  - `AdminApi` HTTP handlers call the same extracted methods — no logic duplication
  - Keep `AdminServer` as optional HTTP wrapper (started only when configured)
- **Update tests:**
  - Test each method returns same results as HTTP endpoint equivalent
  - Test methods work without AdminServer running
  - Verify AdminServer HTTP endpoints still function

**Acceptance Criteria:**

1. `connector.registerPeer(config)` registers a peer with BTP connection and settlement config
2. `connector.removePeer(peerId)` disconnects and removes a peer, returns `RemovePeerResult` with removed route prefixes
3. `connector.listPeers()` returns all connected peers with status
4. `connector.getBalance(peerId)` returns TigerBeetle account balance
5. `connector.listRoutes()` returns routing table entries
6. `connector.addRoute(route)` adds a static route
7. `connector.removeRoute(prefix)` removes a route by prefix
8. All methods work without `AdminServer` running
9. `AdminServer` HTTP endpoints still work when enabled (backward compatible)
10. No logic duplication between methods and HTTP handlers
11. New tests for each method; existing admin API tests unchanged

---

## Compatibility Requirements

- [x] **Existing file-path constructor** — still works for YAML config files
- [x] **HTTP local delivery** — `LocalDeliveryClient` still used when no function handler set
- [x] **Admin HTTP API** — all `AdminServer` endpoints unchanged
- [x] **BTP protocol** — no changes to BTP server/client behavior
- [x] **Outbound send HTTP** — `POST /ilp/send` still functional
- [x] **Config validation** — same validation logic, same error messages

## Risk Mitigation

**Primary Risk:** Breaking existing ConnectorNode initialization or packet flow.

**Mitigation:**

- Constructor is backward compatible (string path still works via type check)
- Function handler is additive — LocalDeliveryClient HTTP is the default fallback
- sendPacket() reuses existing PacketHandler/routing — no new routing logic
- Admin methods extract existing logic — AdminApi handlers become thin wrappers

**Secondary Risk:** Admin method behavior drift from HTTP endpoints.

**Mitigation:**

- Both paths call the same extracted service methods — single source of truth
- Integration tests verify parity between direct methods and HTTP calls

**Rollback Plan:**

1. Revert stories — constructor reverts to file-path-only
2. No configuration changes needed downstream
3. agent-society falls back to HTTP client usage

## Definition of Done

- [ ] All 4 stories completed with acceptance criteria met
- [ ] ConnectorNode accepts config objects and file paths
- [ ] setLocalDeliveryHandler() enables zero-HTTP local delivery
- [ ] sendPacket() exposes packet sending as a public method
- [ ] Admin operations callable as methods without AdminServer
- [ ] All existing tests pass; new tests for each story
- [ ] No TypeScript compilation errors across the monorepo
- [ ] Backward compatible — all HTTP paths still function

## Related Work

- **Epic 20:** Bidirectional Agent-Runtime Middleware (completed — provides `POST /ilp/send`)
- **Epic 21:** Payment Channel Admin APIs (completed — provides channel management endpoints)
- **Epic 22:** Agent-Runtime Middleware Simplification (companion — simplifies the middleware layer)
- **Epic 25:** CLI/Library Separation (depends on this epic — separates lifecycle concerns)
- **Epic 26:** npm Publishing Readiness (depends on this epic — prepares for package publishing)
- **ElizaOS Integration:** agent-society will use these APIs for in-process composition
