# Epic 20: Bidirectional Agent-Runtime Middleware - Brownfield Enhancement

**Epic Number:** 20
**Priority:** High - Foundational enabler for agent-society Epics 7 & 8
**Type:** Middleware Enhancement
**Dependencies:** Epic 19 (deployment parity)

## Epic Goal

Transform agent-runtime from a one-way inbound proxy into a bidirectional middleware so the BLS can both receive and send ILP packets. Add a `POST /ilp/send` endpoint and extend `POST /admin/peers` with settlement configuration fields.

## Epic Description

### Existing System Context

**Current Functionality:**

- Agent-runtime sits between the connector and BLS as a thin middleware
- Inbound flow works: Connector → `POST /ilp/packets` → agent-runtime → `POST /handle-payment` → BLS
- BLS can only respond to incoming payments; it cannot initiate outbound ILP packets
- `POST /admin/peers` accepts id, url, authToken, and optional routes — no settlement fields

**Technology Stack:**

- TypeScript, Express HTTP server (agent-runtime + connector), BTP WebSocket
- `PaymentChannelSDK` for EVM, `XRPChannelSDK` for XRP, `AptosChannelSDK` for Aptos
- `UnifiedSettlementExecutor` with per-peer `PeerConfig` (settlementPreference, chain addresses)

**Integration Points:**

- `packages/agent-runtime/src/http/http-server.ts` — HTTP routes (add `POST /ilp/send`)
- `packages/agent-runtime/src/agent-runtime.ts` — Initialization (add BTP client)
- `packages/agent-runtime/src/types/index.ts` — Type definitions
- `packages/connector/src/http/admin-api.ts` — Admin API (extend AddPeerRequest)
- `packages/connector/src/settlement/types.ts` — PeerConfig interface
- `tools/send-packet/src/btp-sender.ts` — Existing BTP injection pattern to reuse

### Enhancement Details

**What's Being Added:**

1. **`POST /ilp/send` endpoint** on agent-runtime HTTP server — accepts destination, amount, data (base64), and timeoutMs. Builds an ILP PREPARE packet, injects it into the connector via BTP WebSocket, waits for FULFILL/REJECT, and returns the result to the BLS caller.

2. **BTP client in agent-runtime** — Reuses the pattern from `tools/send-packet/src/btp-sender.ts` to connect to the local connector's BTP endpoint and inject outbound packets.

3. **Settlement fields on `POST /admin/peers`** — Extends `AddPeerRequest` with an optional `settlement` object containing preference, chain addresses, token/contract addresses, channelId, and initialDeposit. Wires to `UnifiedSettlementExecutor` PeerConfig.

**How It Integrates:**

- BLS calls `POST /ilp/send` on agent-runtime to initiate any outbound ILP packet
- Agent-runtime computes `executionCondition = SHA256(SHA256(data))` (condition = hash of fulfillment, fulfillment = hash of TOON data)
- Injects PREPARE into connector via BTP, waits for response
- Returns FULFILL data or REJECT error to the BLS
- BLS calls `POST /admin/peers` with settlement config when registering peers discovered via Nostr

**Success Criteria:**

1. BLS can send a 0-amount ILP packet via `POST /ilp/send` and receive a FULFILL response
2. BLS can send a paid ILP packet and receive FULFILL/REJECT
3. `POST /admin/peers` accepts settlement config and creates PeerConfig in UnifiedSettlementExecutor
4. Existing inbound flow (`POST /ilp/packets`) unchanged
5. Integration test: 2-peer network, BLS sends packet via `/ilp/send`, receives fulfill end-to-end

## Stories

### Story 20.1: Add `POST /ilp/send` Endpoint to Agent-Runtime

**As a** BLS developer,
**I want** an HTTP endpoint on agent-runtime that lets me send outbound ILP packets,
**so that** my BLS can initiate communication with other peers (e.g., SPSP handshakes, peer announcements).

**Scope:**

- Add `POST /ilp/send` route to `http-server.ts`
- Request body: `{ destination: string, amount: string, data: string, timeoutMs?: number }`
- Response body (fulfill): `{ fulfilled: true, fulfillment: string, data?: string }`
- Response body (reject): `{ fulfilled: false, code: string, message: string, data?: string }`
- Validate inputs (destination is valid ILP address, amount is non-negative integer string, data is base64)
- Compute `executionCondition = SHA256(SHA256(base64decode(data)))` — condition is hash of fulfillment, fulfillment is SHA256 of the raw data bytes
- Build ILP PREPARE with destination, amount, executionCondition, expiresAt (now + timeoutMs), data
- Pass PREPARE to BTP sender (Story 20.2) and await response
- Map FULFILL/REJECT back to HTTP response

**Acceptance Criteria:**

1. `POST /ilp/send` endpoint registered on agent-runtime HTTP server
2. Request validation rejects invalid ILP addresses, negative amounts, non-base64 data
3. Condition computed as SHA256(SHA256(data)) — matches fulfillment = SHA256(data)
4. Default timeoutMs is 30000 (30 seconds)
5. HTTP 200 returned for both fulfill and reject (distinguished by `fulfilled` boolean)
6. HTTP 408 returned on timeout
7. HTTP 400 returned on validation failure
8. Existing routes (`/ilp/packets`, `/health`) unaffected
9. Unit tests for condition computation, request validation, response mapping
10. OpenAPI/JSDoc for the endpoint

---

### Story 20.2: Implement BTP Client for Outbound Packet Injection

**As a** agent-runtime developer,
**I want** a BTP WebSocket client that can inject ILP PREPARE packets into the local connector,
**so that** outbound packets from `POST /ilp/send` reach the ILP network.

**Scope:**

- Create `OutboundBTPClient` class (or similar) reusing patterns from `tools/send-packet/src/btp-sender.ts`
- Connect to the local connector's BTP endpoint on startup (configurable via `CONNECTOR_BTP_URL` env var)
- Implement `sendPacket(prepare: IlpPrepare): Promise<IlpFulfill | IlpReject>` method
- Handle BTP protocol handshake (AUTH message exchange)
- Handle response correlation (match FULFILL/REJECT to pending PREPARE by request ID)
- Handle connection lifecycle (reconnect on disconnect, health checks)
- Wire into agent-runtime initialization (`agent-runtime.ts`)

**Acceptance Criteria:**

1. `OutboundBTPClient` connects to connector BTP endpoint on startup
2. BTP AUTH handshake completes successfully
3. `sendPacket()` sends PREPARE and returns FULFILL/REJECT
4. Multiple concurrent requests supported (request correlation by ID)
5. Timeout handling: rejects promise after timeoutMs
6. Reconnection on WebSocket disconnect (exponential backoff)
7. Health status exposed (connected/disconnected) via `/health` endpoint
8. `CONNECTOR_BTP_URL` env var documented and validated on startup
9. Graceful shutdown: close WebSocket on SIGTERM
10. Integration test: send packet through 2-peer network, verify fulfill

---

### Story 20.3: Extend `POST /admin/peers` with Settlement Configuration

**As a** BLS developer,
**I want** to pass settlement configuration when registering a peer via the Admin API,
**so that** the connector knows how to settle with that peer on the negotiated chain.

**Scope:**

- Extend `AddPeerRequest` interface in `admin-api.ts` with optional `settlement` object
- Settlement fields: `preference` (evm|xrp|aptos|any), `evmAddress`, `xrpAddress`, `aptosAddress`, `aptosPubkey`, `tokenAddress`, `tokenNetworkAddress`, `chainId`, `channelId`, `initialDeposit`
- On peer creation, if settlement config provided, create `PeerConfig` and register with `UnifiedSettlementExecutor`
- Validate settlement addresses by chain type
- Return settlement config in peer list responses (`GET /admin/peers`)

**Acceptance Criteria:**

1. `AddPeerRequest` accepts optional `settlement` object
2. Settlement fields validated: EVM addresses are 0x-prefixed, XRP addresses start with 'r', Aptos addresses are 0x-prefixed
3. `PeerConfig` created and stored in `UnifiedSettlementExecutor.config.peers` Map
4. `GET /admin/peers` response includes settlement info per peer
5. `DELETE /admin/peers/:peerId` also removes PeerConfig
6. Backward compatible: existing requests without `settlement` still work
7. If channelId provided, it's stored for claim exchange
8. Unit tests for validation, PeerConfig creation, and backward compatibility
9. Integration test: register peer with settlement, verify PeerConfig exists
10. API documentation updated

---

### Story 20.4: Fix IlpSendResponse Field Name and Make Peer Registration Idempotent

**As a** BLS developer,
**I want** the `POST /ilp/send` response to use `accepted` instead of `fulfilled` and `POST /admin/peers` to handle re-registration gracefully,
**so that** agent-society can parse ILP send results correctly and update peer settlement config without 409 errors during bootstrap.

**Scope:**

- Rename `fulfilled` to `accepted` in `IlpSendResponse` (with backward-compat `fulfilled` field kept)
- Make `POST /admin/peers` idempotent: return 200 with merged config on duplicate instead of 409
- Add `PUT /admin/peers/:peerId` for explicit partial updates
- Enhance EVM channel opening to use `settlementPeers` fallback for peer address

**Acceptance Criteria:**

1. `POST /ilp/send` response uses `accepted: true/false` field
2. Response also includes deprecated `fulfilled` field for backward compatibility
3. `POST /admin/peers` returns 200 (not 409) on re-registration, merges settlement and routes
4. `PUT /admin/peers/:peerId` added for explicit updates (404 on unknown peer)
5. EVM channel opening looks up `settlementPeers.get(peerId)?.evmAddress` as fallback
6. Unit tests for all changes

**Priority:** P0 — Critical (bootstrap broken without `accepted` field fix)

---

## Compatibility Requirements

- [x] **Existing APIs remain unchanged** — `POST /ilp/packets` and `/handle-payment` unmodified
- [x] **New endpoint is additive** — `POST /ilp/send` is a new route, no breaking changes
- [x] **Admin API backward compatible** — `settlement` field is optional on `POST /admin/peers`
- [x] **Performance impact minimal** — BTP client adds one WebSocket connection per agent-runtime instance

## Risk Mitigation

**Primary Risk:** BTP client connection stability — if the WebSocket drops, outbound sends fail.

**Mitigation:**

- Exponential backoff reconnection
- Health endpoint reports BTP connection status
- BLS receives clear error response (HTTP 503) when BTP disconnected
- Inbound flow (`POST /ilp/packets`) is unaffected by outbound BTP client state

**Rollback Plan:**

1. Remove `POST /ilp/send` route from http-server.ts
2. Remove OutboundBTPClient from agent-runtime.ts
3. Revert AddPeerRequest to original interface
4. Redeploy — system reverts to inbound-only mode

## Definition of Done

- [ ] All 4 stories completed with acceptance criteria met
- [ ] BLS can send outbound ILP packets via `POST /ilp/send`
- [ ] Admin API accepts settlement configuration on peer registration
- [ ] Existing inbound flow unchanged (no regression)
- [ ] Integration tests pass for outbound send + settlement config
- [ ] Documentation updated (API docs, env vars, architecture diagram)

## Related Work

- **Epic 17:** BTP Off-Chain Claim Exchange (provides BTP sub-protocol patterns)
- **Epic 21:** Payment Channel Admin APIs (extends Admin API with channel management and balance queries — companion epic)
- **Agent-Society Epic 7:** SPSP Settlement Negotiation (consumes `POST /ilp/send`, settlement Admin API, and channel Admin APIs)
- **Agent-Society Epic 8:** Nostr Network Bootstrap (uses all three API surfaces for bootstrap flow)
