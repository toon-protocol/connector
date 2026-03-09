# Epic 25: CLI/Library Separation & Lifecycle Cleanup — Brownfield Enhancement

**Epic Number:** 25
**Priority:** High — Required for safe in-process embedding
**Type:** Package Structure Refactoring
**Dependencies:** Epic 24 (connector library API)

## Epic Goal

Separate the CLI entrypoint from the library exports, remove `process.exit()` calls and signal handlers from library code, export all types needed for in-process composition, and ensure `ConnectorNode` has clean, reentrant lifecycle methods — making `@crosstown/connector` safe to import and embed without side effects.

## Epic Description

### Existing System Context

**Current Functionality:**

- `packages/connector/src/index.ts` serves as **both** the library export point and the application entrypoint — it contains a `main()` function with `process.exit()` calls, SIGTERM/SIGINT signal handlers, and uncaught exception handlers
- `ConnectorNode.start()` and `stop()` exist but `start()` optionally starts `AdminServer` with its own lifecycle, and the overall startup is tied to the `main()` function in `index.ts`
- The CLI (`src/cli/index.ts`) is a Commander-based tool for setup/validation but doesn't start the connector — `main()` in `index.ts` handles actual startup
- Package exports include `ConnectorNode`, `RoutingTable`, `PacketHandler`, `BTPServer`, `BTPClient`, `LocalDeliveryClient`, `createLogger`, and `main` — but are missing key types (`ConnectorConfig`, `PeerConfig`, `RouteConfig`, `SettlementConfig`, ILP packet types)
- `package.json` has `"main": "dist/index.js"` and `"bin": { "agent-runtime": "./dist/cli/index.js" }` — the main entrypoint includes process lifecycle code

**Technology Stack:**

- TypeScript, Node.js process APIs (signals, exit), Commander (CLI), Express (AdminServer)

**Integration Points:**

- `packages/connector/src/index.ts` — Library exports + main() function (needs splitting)
- `packages/connector/src/cli/index.ts` — CLI entrypoint (needs to own process lifecycle)
- `packages/connector/src/core/connector-node.ts` — start()/stop() lifecycle
- `packages/connector/src/config/types.ts` — ConnectorConfig and related types
- `packages/connector/package.json` — main, bin, types, exports fields

### Enhancement Details

**What's Being Changed:**

1. **Split index.ts into library and entrypoint** — `src/index.ts` becomes a pure re-export module (no `main()`, no process lifecycle). A new `src/main.ts` (or `src/cli/start.ts`) owns the `main()` function, signal handlers, `process.exit()`, and config file loading.
2. **Clean lifecycle methods** — `ConnectorNode.start()` and `stop()` are clean and reentrant. No `process.exit()` calls anywhere in library code. No signal handler registration. Those concerns belong exclusively in the CLI/main entrypoint.
3. **Export all composition types** — `ConnectorConfig`, `PeerConfig`, `RouteConfig`, `SettlementConfig`, ILP packet types, `LocalDeliveryHandler`, and all types needed to construct and interact with `ConnectorNode` programmatically.
4. **Package.json structure** — `main` points to library exports, `bin` points to CLI, `exports` field for modern Node.js resolution.

**How It Integrates:**

- `import { ConnectorNode, type ConnectorConfig } from '@crosstown/connector'` — clean import, no side effects
- The CLI (`npx connector start config.yaml`) still works — it imports from the library and adds process lifecycle
- ElizaOS Service imports the library, creates ConnectorNode, manages lifecycle via Service.start()/stop()

**Success Criteria:**

1. `import { ConnectorNode } from '@crosstown/connector'` has zero side effects (no signal handlers, no process listeners)
2. All types needed for programmatic composition are exported
3. `ConnectorNode.start()` and `stop()` are clean — no process.exit(), no signal registration
4. CLI still works for standalone usage
5. Package structure supports both `import` (library) and `npx` (CLI) usage
6. All existing tests pass

## Stories

### Story 25.1: Split Library Exports from Process Entrypoint

**As a** library consumer,
**I want** importing `@crosstown/connector` to have zero side effects,
**so that** I can safely embed it in my process without unexpected signal handlers or exit behavior.

**Scope:**

- **Create `src/lib.ts`** (or rename current `src/index.ts`):
  - Pure re-exports only — no executable code
  - Exports all classes, types, and utilities needed for library consumption
  - No `main()` function, no `process.exit()`, no signal handlers
- **Move `main()` to `src/main.ts`** (or `src/cli/start.ts`):
  - Contains `main()` function with config file loading via `ConfigLoader`
  - Contains SIGTERM, SIGINT signal handlers
  - Contains `process.exit()` calls
  - Contains uncaught exception and unhandled rejection handlers
  - Imports `ConnectorNode` and `createLogger` from the library
- **Update `package.json`:**
  - `"main": "./dist/lib.js"` — points to side-effect-free library
  - `"types": "./dist/lib.d.ts"`
  - `"bin": { "agent-runtime": "./dist/main.js" }` — or `./dist/cli/start.js`
  - Add `"exports"` field:
    ```json
    {
      ".": {
        "import": "./dist/lib.js",
        "types": "./dist/lib.d.ts"
      }
    }
    ```
- **Update tsconfig/build:**
  - Ensure both `lib.ts` and `main.ts` are compiled
  - `main.ts` gets `#!/usr/bin/env node` shebang for CLI usage

**Acceptance Criteria:**

1. `import { ConnectorNode } from '@crosstown/connector'` imports from `lib.ts` — zero side effects
2. No `process.exit()`, `process.on('SIGTERM')`, or `process.on('uncaughtException')` in library code
3. CLI entrypoint (`main.ts`) handles all process lifecycle concerns
4. `npx connector` or direct execution of `main.js` works as before
5. Package `"main"` and `"exports"` point to side-effect-free library
6. TypeScript compilation succeeds
7. All existing tests pass (import paths may need updating)

---

### Story 25.2: Clean ConnectorNode Lifecycle Methods

**As a** library consumer,
**I want** `ConnectorNode.start()` and `stop()` to be clean and reentrant,
**so that** I can safely manage connector lifecycle within my own process without unexpected exit behavior.

**Scope:**

- **Audit `ConnectorNode.start()`:**
  - Remove any `process.exit()` calls (move to CLI entrypoint)
  - Remove any signal handler registration (move to CLI entrypoint)
  - Ensure `start()` throws on failure instead of calling `process.exit(1)`
  - Ensure `start()` can be called, stopped, and called again (reentrant)
  - Log errors and propagate them — don't swallow and exit
- **Audit `ConnectorNode.stop()`:**
  - Gracefully close BTP connections
  - Stop AdminServer (if running)
  - Stop SettlementMonitor (if running)
  - Close TigerBeetle client (if connected)
  - Resolve cleanly — no `process.exit()` on completion
  - Idempotent — calling `stop()` twice is safe
- **Audit `AdminServer`:**
  - `start()` and `stop()` methods should not reference process lifecycle
  - Should throw on port binding failure, not exit
- **Update tests:**
  - Test start → stop → start cycle (reentrant)
  - Test stop() idempotency
  - Test start() failure propagation (throws, not exits)

**Acceptance Criteria:**

1. Zero `process.exit()` calls in `ConnectorNode`, `AdminServer`, or any non-CLI code
2. Zero `process.on(signal)` calls in library code
3. `start()` throws on failure (not exits) — caller handles the error
4. `stop()` is idempotent — safe to call multiple times
5. `start()` → `stop()` → `start()` works (reentrant)
6. BTP connections, AdminServer, SettlementMonitor all shut down gracefully on `stop()`
7. All existing lifecycle tests pass
8. New tests for reentrant lifecycle and error propagation

---

### Story 25.3: Export All Composition Types

**As a** library consumer,
**I want** all types needed for in-process composition to be exported,
**so that** I can construct and interact with `ConnectorNode` with full type safety.

**Scope:**

- **Export from library entry point (`lib.ts`):**
  - **Core classes:** `ConnectorNode`, `PacketHandler`, `RoutingTable`
  - **BTP:** `BTPServer`, `BTPClient`, `BTPClientManager`
  - **Settlement:** `AccountManager`, `SettlementMonitor`, `SettlementExecutor`
  - **Admin:** `AdminServer` (optional HTTP wrapper)
  - **Local delivery:** `LocalDeliveryClient` (HTTP fallback)
  - **Logger:** `createLogger`
  - **Config:** `ConfigLoader` (for `validateConfig()`)
- **Export types:**
  - `ConnectorConfig`, `PeerConfig`, `RouteConfig`, `SettlementConfig`, `LocalDeliveryConfig`
  - `LocalDeliveryHandler` (function handler type from Epic 24 Story 24.2)
  - `PeerRegistrationRequest`, `PeerInfo`, `PeerAccountBalance`, `RouteInfo`
  - ILP packet types: `ILPPreparePacket`, `ILPFulfillPacket`, `ILPRejectPacket` (re-exported from `@crosstown/shared`)
  - `SendPacketParams`, `SendPacketResult` (from Epic 24 Story 24.3)
- **Verify completeness:**
  - Write a TypeScript "consumer test" file that imports all exported types and attempts to construct a ConnectorNode with full type safety
  - Ensure no `any` types needed — all parameters and returns are fully typed

**Acceptance Criteria:**

1. All classes listed above are exported from library entry point
2. All types listed above are exported from library entry point
3. Consumer test file compiles with strict TypeScript — no `any` casts needed
4. ILP packet types re-exported from `@crosstown/shared` for convenience
5. No internal-only types leaked (implementation details stay private)
6. `npm pack` includes all `.d.ts` files for exported types
7. TypeScript compilation succeeds across the monorepo

---

## Compatibility Requirements

- [x] **CLI still works** — `npx connector` or direct `node dist/main.js` unchanged
- [x] **Existing imports** — any internal consumers of `@crosstown/connector` still resolve (re-exports cover existing API)
- [x] **Test imports** — existing test files import paths updated if needed
- [x] **Docker deployments** — Dockerfile CMD still starts the connector correctly
- [x] **No behavior changes** — same connector behavior, just different entry/exit structure

## Risk Mitigation

**Primary Risk:** Breaking the CLI startup or Docker deployments by moving main().

**Mitigation:**

- The CLI entrypoint imports from the library and adds lifecycle — same code, different file
- `package.json` `bin` field updated to point to new entrypoint
- Docker CMD verified to still work after path change

**Secondary Risk:** Missing type exports causing compilation failures for consumers.

**Mitigation:**

- Consumer test file validates all needed types are accessible
- `npm pack` + install test in clean project verifies published types resolve

**Rollback Plan:**

1. Revert file moves — merge `main.ts` back into `index.ts`
2. Revert `package.json` changes
3. No downstream behavioral impact

## Definition of Done

- [ ] All 3 stories completed with acceptance criteria met
- [ ] Library import has zero side effects (no signal handlers, no process listeners)
- [ ] ConnectorNode lifecycle is clean, reentrant, and idempotent
- [ ] All composition types exported with full TypeScript type safety
- [ ] CLI still works for standalone usage
- [ ] Docker deployments still work
- [ ] All existing tests pass; new tests for lifecycle and type completeness
- [ ] No TypeScript compilation errors across the monorepo

## Related Work

- **Epic 24:** Connector Library API (prerequisite — provides the methods and types to export)
- **Epic 26:** npm Publishing Readiness (depends on this epic — publishes the properly structured package)
- **Epic 22:** Agent-Runtime Middleware Simplification (companion — simplifies what the middleware exports)
- **ElizaOS Integration:** This epic ensures the package is safe to import in ElizaOS Service
