# Epic 22: Agent-Runtime Middleware Simplification — Brownfield Enhancement

**Epic Number:** 22
**Priority:** High — Prerequisite for unified deployment (Phase 1 of UNIFIED-DEPLOYMENT-PLAN.md)
**Type:** Middleware Refactoring
**Dependencies:** Epic 20 (bidirectional middleware — completed)

## Epic Goal

Strip STREAM session management, SPSP HTTP endpoints, and HMAC-based fulfillment from the agent-runtime middleware, replacing the fulfillment model with simple `SHA256(data)`. This transforms agent-runtime into a thin bidirectional forwarder where the BLS (agent-society) owns all SPSP/STREAM concerns via Nostr (kind:23194/23195).

## Epic Description

### Existing System Context

**Current Functionality:**

- Agent-runtime middleware sits between the connector and BLS, handling inbound packets (`POST /ilp/packets`) and outbound packets (`POST /ilp/send`, Epic 20)
- `PacketHandler` looks up a `PaymentSession` by destination address via `SessionManager.getSessionByAddress()`, verifies the STREAM execution condition via `verifyCondition(session.sharedSecret, prepareData, condition)`, then computes fulfillment via `computeFulfillment(session.sharedSecret, prepareData)` — a two-level HMAC-SHA256 chain per RFC-0029
- `SessionManager` maintains an in-memory Map of payment sessions with shared secrets, TTL expiration, and periodic cleanup
- `SPSPServer` exposes `GET /.well-known/pay/:paymentId?` and `GET /pay/:paymentId?` endpoints per RFC-0009, creating sessions and returning SPSP responses (`destination_account`, `shared_secret`)
- `HttpServer` mounts SPSP routes via `this.app.use(this.spspServer.getRouter())`
- `AgentRuntime` orchestrator initializes `SessionManager`, `SPSPServer`, `PacketHandler`, `BusinessClient`, `HttpServer`, and `OutboundBTPClient`
- Types include `PaymentSession`, `SPSPResponse`, `PaymentSetupRequest`, `PaymentSetupResponse`; config includes `spspEnabled`, `sessionTtlMs`

**Technology Stack:**

- TypeScript, Express HTTP server, pino logging
- STREAM fulfillment: `HMAC-SHA256(HMAC-SHA256(sharedSecret, "ilp_stream_fulfillment"), prepareData)` per RFC-0029
- Agent-society handles SPSP via Nostr events (kind:23194/23195 with NIP-44 encryption), not HTTP

**Integration Points:**

- `packages/agent-runtime/src/packet/packet-handler.ts` — Remove SessionManager dependency, replace fulfillment model
- `packages/agent-runtime/src/stream/fulfillment.ts` — Replace STREAM HMAC chain with `SHA256(data)`
- `packages/agent-runtime/src/session/session-manager.ts` — Remove entirely
- `packages/agent-runtime/src/spsp/spsp-server.ts` — Remove entirely
- `packages/agent-runtime/src/http/http-server.ts` — Remove SPSP routes and SessionManager dependency
- `packages/agent-runtime/src/agent-runtime.ts` — Remove SessionManager/SPSP initialization, simplify config resolution
- `packages/agent-runtime/src/types/index.ts` — Remove STREAM/SPSP types and config fields

### Enhancement Details

**What's Being Changed:**

The agent-runtime middleware currently handles SPSP (payment setup) and STREAM (fulfillment computation) — protocols that agent-society now owns via Nostr. With agent-society handling SPSP through kind:23194/23195 events and settlement negotiation via the connector Admin API, agent-runtime no longer needs sessions, shared secrets, or STREAM fulfillment. The middleware becomes a thin forwarder:

1. **Inbound** (`POST /ilp/packets`): Receive ILP Prepare from connector → forward to BLS via `POST /handle-payment` → if accepted, compute `fulfillment = SHA256(data)` → return FULFILL to connector
2. **Outbound** (`POST /ilp/send`): Already implemented (Epic 20) — BLS sends ILP packets through connector

The simplified fulfillment model (`SHA256(data)`) ties the fulfillment to the exact TOON bytes transmitted, making it deterministic without shared secrets. The sender computes `condition = SHA256(SHA256(data))` (already implemented in `ilp-send-handler.ts`), and the receiver computes `fulfillment = SHA256(data)` which the connector verifies as `SHA256(fulfillment) == condition`.

**How It Integrates:**

- Inbound flow becomes: connector → `POST /ilp/packets` → PacketHandler builds PaymentRequest → BLS `POST /handle-payment` returns accept/reject → if accepted, `fulfillment = SHA256(base64decode(data))` → FULFILL returned to connector
- No session lookup — PacketHandler accepts any destination under the base address
- No condition verification — the connector already verifies `SHA256(fulfillment) == condition` on the FULFILL path
- Outbound flow unchanged (Epic 20 `POST /ilp/send` already uses `SHA256(SHA256(data))` for condition)
- BLS response `data` field passed through into ILP FULFILL/REJECT data for application-level responses

**Success Criteria:**

1. PacketHandler no longer requires SessionManager — accepts packets for any destination under base address
2. Fulfillment computed as `SHA256(base64decode(request.data))` — no shared secrets needed
3. SPSP HTTP endpoints removed — agent-society handles SPSP via Nostr
4. SessionManager class and tests removed
5. Existing outbound flow (`POST /ilp/send`) unchanged
6. Existing connector integration (`POST /ilp/packets` contract) unchanged — same `LocalDeliveryRequest`/`LocalDeliveryResponse` format
7. Health endpoint no longer reports `activeSessions`
8. All remaining tests pass; new unit tests for simplified fulfillment

## Stories

### Story 22.1: Simplify PacketHandler to Use SHA256(data) Fulfillment

**As a** connector operator,
**I want** the agent-runtime to compute fulfillment as SHA256(data) without session lookup,
**so that** the middleware is stateless and doesn't require SPSP session setup before receiving payments.

**Scope:**

- **Modify `packet-handler.ts`:**
  - Remove `SessionManager` from constructor dependencies
  - Remove `session = this.sessionManager.getSessionByAddress(destination)` lookup and session-not-found rejection
  - Remove `verifyCondition(session.sharedSecret, prepareData, condition)` check
  - Generate a deterministic `paymentId` from the packet (e.g., SHA256 of `destination + amount + data` truncated, or use a UUID)
  - Build `PaymentRequest` directly from the `LocalDeliveryRequest` fields (no session metadata)
  - On accept: compute `fulfillment = SHA256(Buffer.from(request.data, 'base64'))`
  - On reject: pass through BLS `data` field into reject response (currently missing — the UNIFIED-DEPLOYMENT-PLAN requires BLS data pass-through on both accept and reject)
- **Modify `fulfillment.ts`:**
  - Replace all STREAM functions (`deriveFulfillmentKey`, `computeFulfillment`, `computeCondition`, `computeExpectedCondition`, `verifyCondition`) with a single `computeFulfillmentFromData(data: Buffer): Buffer` that returns `SHA256(data)`
  - Keep `generatePaymentId()` (still useful for ID generation)
  - Remove `generateSharedSecret()` (no longer needed)
- **Update/create tests:**
  - New unit tests for `computeFulfillmentFromData`: verify SHA256 output matches expected hash
  - New unit tests for PacketHandler: verify packets accepted without session, fulfillment = SHA256(data), BLS data passed through on accept and reject
  - Remove `session-manager.test.ts`
  - Update `fulfillment.test.ts` to cover new function

**Acceptance Criteria:**

1. PacketHandler constructor no longer takes SessionManager parameter
2. Any packet with a valid destination under base address is forwarded to BLS (no session required)
3. Fulfillment computed as `SHA256(base64decode(request.data))` — verified by unit test
4. Fulfillment matches condition from outbound sender: `SHA256(fulfillment) == SHA256(SHA256(data))` ✓
5. BLS `data` field passed through in both FULFILL and REJECT responses
6. Payment expiry check retained (`expiresAt < now` → reject R00)
7. `fulfillment.test.ts` updated with new SHA256-based tests
8. `session-manager.test.ts` removed
9. No regression in outbound send tests (`ilp-send-handler.test.ts`, `outbound-btp-send.test.ts`)

---

### Story 22.2: Remove SPSP Server and Session Manager

**As a** developer,
**I want** SPSP HTTP endpoints and session management removed from agent-runtime,
**so that** the middleware is a clean forwarder without dead code from the STREAM/SPSP protocol layer.

**Scope:**

- **Delete `src/session/session-manager.ts`** — entire file (205 lines)
- **Delete `src/session/session-manager.test.ts`** — entire test file
- **Delete `src/spsp/spsp-server.ts`** — entire file (140 lines)
- **Modify `src/http/http-server.ts`:**
  - Remove `SPSPServer` import and constructor parameter
  - Remove `SessionManager` import and constructor parameter
  - Remove `this.app.use(this.spspServer.getRouter())` SPSP route mount
  - Remove `activeSessions: this.sessionManager.sessionCount` from health endpoint
  - Simplify health response to: `{ status, nodeId, btpConnected, timestamp }`
- **Modify `src/agent-runtime.ts`:**
  - Remove `SessionManager` import and instantiation
  - Remove `SPSPServer` import and instantiation
  - Remove `SessionManager` from `HttpServer` and `PacketHandler` constructor calls
  - Remove `SPSPServer` from `HttpServer` constructor call
  - Remove `getSessionManager()` public method
  - Remove `this.sessionManager.shutdown()` from `stop()`
  - Remove `spspEnabled` and `sessionTtlMs` from config resolution and env var parsing
  - Remove `SPSP_ENABLED` and `SESSION_TTL_MS` env var handling in `startFromEnv()`

**Acceptance Criteria:**

1. `src/session/` directory removed (or empty)
2. `src/spsp/` directory removed (or empty)
3. No imports of `SessionManager` or `SPSPServer` anywhere in `packages/agent-runtime/src/`
4. `GET /.well-known/pay` and `GET /pay` return 404 (caught by existing 404 handler)
5. `GET /health` no longer includes `activeSessions` field
6. `AgentRuntime` constructor no longer creates SessionManager or SPSPServer
7. `AgentRuntime.stop()` no longer calls `sessionManager.shutdown()`
8. TypeScript compilation succeeds with no errors
9. All remaining tests pass

---

### Story 22.3: Clean Up Types and Configuration

**As a** developer,
**I want** STREAM/SPSP types and config fields removed from the type definitions,
**so that** the codebase accurately reflects the simplified middleware architecture.

**Scope:**

- **Modify `src/types/index.ts`:**
  - Remove `PaymentSession` interface (lines 14-27)
  - Remove `SPSPResponse` interface (lines 114-119)
  - Remove `PaymentSetupRequest` interface (lines 125-130)
  - Remove `PaymentSetupResponse` interface (lines 135-144)
  - Remove `spspEnabled` from `AgentRuntimeConfig` interface
  - Remove `sessionTtlMs` from `AgentRuntimeConfig` interface
  - Remove `spspEnabled` and `sessionTtlMs` from `DEFAULT_CONFIG`
  - Add `connectorBtpUrl` to `AgentRuntimeConfig` (document the BTP URL config that `startFromEnv()` already reads from `CONNECTOR_BTP_URL`)
  - Update JSDoc on `AgentRuntimeConfig` to reflect simplified middleware role
- **Modify `src/business/business-client.ts`:**
  - Remove `paymentSetup()` method if it exists (SPSP setup hook no longer needed)
  - Verify `handlePayment()` accepts the simplified `PaymentRequest` (no metadata field needed, but keep for backward compatibility with BLS)
- **Update `src/index.ts`** (package entry point):
  - Remove exports of `PaymentSession`, `SPSPResponse`, `PaymentSetupRequest`, `PaymentSetupResponse`, `SessionManager`, `SPSPServer`
  - Ensure `computeFulfillmentFromData` is exported (from Story 22.1)
- **Remove `generateSharedSecret` export** from fulfillment.ts (if not done in Story 22.1)

**Acceptance Criteria:**

1. `PaymentSession`, `SPSPResponse`, `PaymentSetupRequest`, `PaymentSetupResponse` no longer in types
2. `AgentRuntimeConfig` no longer has `spspEnabled` or `sessionTtlMs` fields
3. `DEFAULT_CONFIG` no longer has `spspEnabled` or `sessionTtlMs`
4. Package entry point (`src/index.ts`) no longer exports removed types/classes
5. `connectorBtpUrl` documented in `AgentRuntimeConfig` JSDoc
6. TypeScript compilation succeeds with no errors across the monorepo
7. No downstream consumers reference removed types (verify with `grep` across packages/)
8. `PaymentRequest` interface retained — BLS still receives `paymentId`, `destination`, `amount`, `expiresAt`, `data`
9. `PaymentResponse`, `LocalDeliveryRequest`, `LocalDeliveryResponse`, `IlpSendRequest`, `IlpSendResponse`, `IPacketSender`, `REJECT_CODE_MAP` all retained

### Story 22.4: Validate BLS Response Data Before FULFILL Pass-Through

**As a** connector operator,
**I want** the agent-runtime middleware to validate BLS response data before passing it into ILP FULFILL packets,
**so that** malformed or oversized data from the BLS doesn't produce invalid ILP packets.

**Scope:**

- Validate BLS `response.data` is valid base64 before FULFILL/REJECT pass-through
- Validate decoded data size ≤ 32KB (ILP maximum)
- Invalid/oversized data omitted with warning log (payment still fulfills)
- Pure validation utility function for testability

**Acceptance Criteria:**

1. Invalid base64 data omitted from FULFILL with warning
2. Oversized data (> 32KB) omitted with warning
3. Validation applied to both FULFILL and REJECT paths
4. Unit tests for all validation paths
5. No change to fulfillment computation

**Priority:** P2 — Medium (safety)

---

## Compatibility Requirements

- [x] **Connector contract unchanged** — `POST /ilp/packets` still accepts `LocalDeliveryRequest`, returns `LocalDeliveryResponse` with same fulfill/reject structure
- [x] **BLS contract unchanged** — `POST /handle-payment` still receives `PaymentRequest`, returns `PaymentResponse` with same accept/reject structure
- [x] **Outbound send unchanged** — `POST /ilp/send` (Epic 20) unmodified
- [x] **Fulfillment model compatible** — sender uses `condition = SHA256(SHA256(data))` (already in `ilp-send-handler.ts`), receiver uses `fulfillment = SHA256(data)`, connector verifies `SHA256(fulfillment) == condition` ✓
- [x] **Health endpoint simplified** — removes `activeSessions` (non-breaking for monitoring)

## Risk Mitigation

**Primary Risk:** Breaking the inbound packet flow by removing session-dependent behavior.

**Mitigation:**

- The BLS (agent-society) already handles SPSP via Nostr events — no HTTP SPSP callers to break
- The connector sends packets to `POST /ilp/packets` regardless of session state — it doesn't depend on SPSP
- Unit tests verify the new fulfillment model matches the outbound condition computation
- Integration test (existing `outbound-btp-send.test.ts`) validates end-to-end flow

**Secondary Risk:** Breaking agent-society BLS compatibility if PaymentRequest shape changes.

**Mitigation:**

- `PaymentRequest` interface is preserved (paymentId, destination, amount, expiresAt, data)
- The `metadata` field becomes always-undefined (not removed from interface) — BLS already handles missing metadata
- BLS response `data` field is now passed through on rejects (additive, not breaking)

**Rollback Plan:**

1. Revert the 3 stories (git revert) — restores STREAM/SPSP code
2. Redeploy agent-runtime — system reverts to session-based fulfillment
3. No connector or BLS changes needed for rollback

## Definition of Done

- [ ] All 4 stories completed with acceptance criteria met
- [ ] PacketHandler uses `SHA256(data)` fulfillment without session lookup
- [ ] SPSP server and session manager removed
- [ ] Types cleaned up — no STREAM/SPSP interfaces exported
- [ ] Existing outbound send flow (`POST /ilp/send`) unchanged
- [ ] Connector contract (`LocalDeliveryRequest`/`LocalDeliveryResponse`) unchanged
- [ ] All remaining unit and integration tests pass
- [ ] No TypeScript compilation errors across the monorepo

## Related Work

- **Epic 20:** Bidirectional Agent-Runtime Middleware (completed — `POST /ilp/send` already uses `SHA256(SHA256(data))` condition model)
- **Epic 21:** Payment Channel Admin APIs (companion — exposes channel management for BLS)
- **Epic 23:** Unified Deployment Infrastructure (depends on this epic — unified Docker Compose expects simplified middleware)
- **Agent-Society Epic 7:** SPSP Settlement Negotiation (owns SPSP via Nostr — no longer depends on agent-runtime SPSP)
- **Agent-Society Phase 2:** BLS Response Simplification (removes fulfillment from BLS responses, nests rejects under `rejectReason`)
- **UNIFIED-DEPLOYMENT-PLAN.md Phase 1:** This epic implements Phase 1 of the unified deployment plan
