# Epic 21: Payment Channel Admin APIs

**Epic Number:** 21
**Priority:** High - Required by agent-society Epics 7 & 8
**Type:** Admin API Enhancement
**Dependencies:** Epic 17 (BTP claim exchange), Epic 19 (deployment parity)

## Epic Goal

Expose payment channel management and balance query endpoints on the connector Admin API so that an external BLS can open, close, fund, and inspect payment channels without direct blockchain SDK access. The BLS makes business decisions (which chain, which peer); the connector executes on-chain operations.

## Epic Description

### Existing System Context

**Current Functionality:**

- The connector has a rich internal settlement layer: `ChannelManager`, `PaymentChannelSDK` (EVM), `XRPChannelSDK`, `AptosChannelSDK`, `ClaimSender`, `ClaimReceiver`, `AccountManager`, `SettlementMonitor`
- The Admin API (`/admin`) currently only exposes peer and route CRUD (6 endpoints)
- All channel operations are internal — triggered automatically by `SettlementMonitor` threshold crossings or by `UnifiedSettlementExecutor`
- The BLS has no way to programmatically open a channel, check a channel's state, or query balances

**Technology Stack:**

- TypeScript, Hono HTTP server, Express-style Admin API
- `ChannelManager` wraps `PaymentChannelSDK` for EVM channel lifecycle
- `XRPChannelManager` wraps `XRPChannelSDK` for XRP channels
- `AptosChannelSDK` for Aptos channels
- `AccountManager` uses TigerBeetle for double-entry accounting
- `SettlementMonitor` polls balances and emits `SETTLEMENT_REQUIRED` events

**Integration Points:**

- `packages/connector/src/http/admin-api.ts` — Add new route handlers
- `packages/connector/src/settlement/channel-manager.ts` — `ensureChannelExists()`, `getChannelForPeer()`, `getAllChannels()`, `getChannelById()`
- `packages/connector/src/settlement/payment-channel-sdk.ts` — `openChannel()`, `deposit()`, `closeChannel()`, `cooperativeSettle()`, `getChannelState()`
- `packages/connector/src/settlement/account-manager.ts` — `getAccountBalance()`, `getPeerAccountPair()`
- `packages/connector/src/settlement/claim-receiver.ts` — `getLatestVerifiedClaim()`
- `packages/connector/src/settlement/settlement-monitor.ts` — `getSettlementState()`, `getAllSettlementStates()`
- `packages/connector/src/core/connector-node.ts` — Wire new API handlers to existing infrastructure

### Enhancement Details

**What's Being Added:**

1. **Channel CRUD endpoints** — Open, list, and inspect payment channels
2. **Channel lifecycle endpoints** — Deposit funds and close channels
3. **Balance query endpoint** — Query peer account balances from TigerBeetle
4. **Settlement state endpoint** — Query settlement monitor state

**How It Integrates:**

- BLS calls these endpoints during SPSP settlement negotiation (agent-society Epic 7)
- BLS calls these endpoints during bootstrap flow (agent-society Epic 8)
- All endpoints delegate to existing internal infrastructure — no new blockchain logic
- Admin API authentication (optional API key) applies to all new endpoints
- Existing automatic settlement flow (SettlementMonitor → UnifiedSettlementExecutor) is unaffected

**Success Criteria:**

1. BLS can open a payment channel via `POST /admin/channels` and receive a channelId
2. BLS can verify a channel is open via `GET /admin/channels/:channelId`
3. BLS can fund a channel via `POST /admin/channels/:channelId/deposit`
4. BLS can close a channel via `POST /admin/channels/:channelId/close`
5. BLS can query peer balances via `GET /admin/balances/:peerId`
6. Existing Admin API endpoints unchanged (backward compatible)
7. Existing automatic settlement flow unaffected

## Stories

### Story 21.1: Payment Channel CRUD Endpoints

**As a** BLS developer,
**I want** to open, list, and inspect payment channels via the Admin API,
**so that** my BLS can manage channel lifecycle during SPSP settlement negotiation.

**Scope:**

- `POST /admin/channels` — Open a new payment channel
  - Request: `{ peerId: string, chain: string, token?: string, tokenNetwork?: string, initialDeposit: string, settlementTimeout?: number }`
  - Chain routing: `evm:*` → `PaymentChannelSDK.openChannel()`, `xrp:*` → `XRPChannelManager.createChannel()`, `aptos:*` → `AptosChannelSDK.openChannel()`
  - Synchronous: blocks until on-chain confirmation, returns channelId
  - Response: `{ channelId: string, chain: string, status: "open", deposit: string }`
- `GET /admin/channels` — List all channels
  - Response: array of channel summaries (channelId, peerId, chain, status, deposit, lastActivity)
  - Optional query filters: `?peerId=`, `?chain=`, `?status=`
- `GET /admin/channels/:channelId` — Get channel details
  - Response: full channel state from SDK (channelId, participants, deposit, transferred, status, nonce, settleTimeout)
  - Includes on-chain state query for freshness

**Acceptance Criteria:**

1. `POST /admin/channels` opens an EVM channel via `ChannelManager.ensureChannelExists()` and returns channelId
2. `POST /admin/channels` opens XRP/Aptos channels via respective managers
3. Chain routing selects correct SDK based on chain identifier prefix
4. Request validation: chain format matches `{blockchain}:{network}:{chainId}`, deposit is non-negative integer string
5. `GET /admin/channels` returns all channels from `ChannelManager.getAllChannels()`
6. `GET /admin/channels/:channelId` returns on-chain state from `PaymentChannelSDK.getChannelState()`
7. 404 returned for unknown channelId
8. 400 returned for invalid chain format or missing required fields
9. 409 returned if channel already exists for the same peer+chain+token combination
10. Existing `/admin/peers` and `/admin/routes` endpoints unaffected
11. Admin API key authentication applies to new endpoints
12. Unit tests for request validation, chain routing, response mapping
13. Integration test: open EVM channel against Anvil, verify state via GET

---

### Story 21.2: Channel Lifecycle Endpoints (Deposit and Close)

**As a** BLS developer,
**I want** to deposit funds into a channel and close channels via the Admin API,
**so that** my BLS can fund channels after opening and gracefully close relationships.

**Scope:**

- `POST /admin/channels/:channelId/deposit` — Add funds to an existing channel
  - Request: `{ amount: string, token?: string }`
  - Delegates to `PaymentChannelSDK.deposit()` / XRP / Aptos equivalents
  - Synchronous: blocks until on-chain confirmation
  - Response: `{ channelId: string, newDeposit: string, status: "open" }`
- `POST /admin/channels/:channelId/close` — Initiate channel close
  - Request: `{ cooperative?: boolean }` (default: true — attempt cooperative close first)
  - Cooperative: `PaymentChannelSDK.cooperativeSettle()` if both parties have claims
  - Unilateral: `PaymentChannelSDK.closeChannel()` with latest balance proof
  - Response: `{ channelId: string, status: "closing" | "settled", txHash?: string }`

**Acceptance Criteria:**

1. `POST /admin/channels/:channelId/deposit` adds funds to EVM channel
2. `POST /admin/channels/:channelId/deposit` adds funds to XRP/Aptos channels
3. Deposit amount validated as positive integer string
4. 404 returned for unknown channelId
5. 400 returned if channel is not in `open` state (can't deposit to closed channel)
6. `POST /admin/channels/:channelId/close` attempts cooperative close by default
7. Falls back to unilateral close if cooperative close fails
8. Close returns immediately with `status: "closing"` (settlement may take time)
9. Optional `cooperative: false` forces unilateral close
10. Unit tests for deposit validation, close mode selection
11. Integration test: deposit to EVM channel, close cooperatively, verify settled state

---

### Story 21.3: Balance and Settlement State Query Endpoints

**As a** BLS developer,
**I want** to query peer account balances and settlement state via the Admin API,
**so that** my BLS can make pricing decisions and monitor settlement health.

**Scope:**

- `GET /admin/balances/:peerId` — Query balance for a specific peer
  - Delegates to `AccountManager.getAccountBalance(peerId, tokenId)`
  - Response: `{ peerId: string, balances: [{ tokenId: string, debitBalance: string, creditBalance: string, netBalance: string }] }`
  - If `?tokenId=` query param provided, filter to specific token
- `GET /admin/settlement/states` — Query all settlement monitor states
  - Delegates to `SettlementMonitor.getAllSettlementStates()`
  - Response: array of `{ peerId: string, tokenId: string, state: "idle" | "pending" | "in_progress", lastSettlement?: string }`
- `GET /admin/channels/:channelId/claims` — Get latest claim for a channel
  - Delegates to `ClaimReceiver.getLatestVerifiedClaim()`
  - Response: `{ peerId: string, chain: string, channelId: string, amount?: string, nonce?: number, signature?: string }` or 404

**Acceptance Criteria:**

1. `GET /admin/balances/:peerId` returns balance from TigerBeetle via AccountManager
2. Balance response includes both debit and credit balances plus net
3. 404 returned for unknown peerId (no accounts exist)
4. `?tokenId=` filter works correctly
5. `GET /admin/settlement/states` returns all settlement states from SettlementMonitor
6. Settlement state reflects current status accurately (idle/pending/in_progress)
7. `GET /admin/channels/:channelId/claims` returns latest verified claim
8. 404 returned when no claims exist for the channel
9. Unit tests for balance computation, state mapping, claim retrieval
10. Integration test: forward packets to generate balance, verify via API

---

### Story 21.4: Channel Opening Integration Fixes — peerAddress Propagation, Validation, and Error Handling

**As a** BLS developer,
**I want** `POST /admin/channels` to propagate the `peerAddress` parameter through to the SDK, validate peer existence, and return accurate channel status,
**so that** the BLS can open payment channels during SPSP handshakes without circular dependency on prior peer settlement registration.

**Scope:**

- Fix `openChannelForPeer()` to use `options.peerAddress` (core fix — currently ignores it)
- Update XRP path to use `body.peerAddress` with `peerConfig?.xrpAddress` fallback (EVM path already done)
- Validate peer exists before opening channels (404 if unknown)
- Return actual channel status from metadata, not hardcoded `'open'`
- Add peerAddress format validation (chain-specific, after routing)
- Note: `OpenChannelRequest.peerAddress` and `ChannelOpenOptions.peerAddress` already exist

**Acceptance Criteria:**

1. `openChannelForPeer()` resolves address from `options.peerAddress` first, falls back to config map
2. XRP path uses `body.peerAddress` with `peerConfig?.xrpAddress` fallback
3. Peer existence validated — 404 if unknown peerId
4. Actual channel status returned (not hardcoded)
5. peerAddress format validation (EVM and XRP)
6. Unit tests for all new paths

**Priority:** P0/P1 — Critical + High (channel opening broken without `peerAddress` propagation)

---

### Story 21.5: Standardize Channel Status Enum and Response Types

**As a** BLS developer,
**I want** channel status values and response types to be consistent between agent-runtime and agent-society,
**so that** the BLS can reliably parse channel state responses and make correct decisions based on status transitions.

**Scope:**

- Define canonical `ChannelStatus` type: `'opening' | 'open' | 'closing' | 'closed' | 'settled'`
- Normalize `'active'` → `'open'` in all API responses
- Document `OpenChannelResponse` as superset of agent-society's `OpenChannelResult`
- Unit tests for status normalization

**Acceptance Criteria:**

1. Canonical `ChannelStatus` enum defined and used in all channel endpoints
2. `'active'` normalized to `'open'` in API responses
3. Response types documented with agent-society mapping
4. Unit tests for normalization

**Priority:** P2 — Medium (polish)

---

## Compatibility Requirements

- [x] **Existing Admin API unchanged** — `/admin/peers` and `/admin/routes` unmodified
- [x] **New endpoints are additive** — no breaking changes to existing API
- [x] **Admin API key applies** — new endpoints use same auth as existing ones
- [x] **Automatic settlement unaffected** — SettlementMonitor + UnifiedSettlementExecutor flow unchanged
- [x] **No new blockchain dependencies** — all endpoints delegate to existing SDKs

## Risk Mitigation

**Primary Risk:** Synchronous channel opening via API could timeout on slow chains.

**Mitigation:**

- Base L2 confirmation ~2-4s — well within HTTP timeout
- XRP confirmation ~4s — well within HTTP timeout
- Configurable HTTP timeout on the connector Admin API (default 60s)
- BLS can set appropriate ILP packet expiresAt to account for channel opening time
- If timeout occurs, channel may still be opening — BLS can poll `GET /admin/channels/:channelId`

**Secondary Risk:** Concurrent channel opens for the same peer could create duplicates.

**Mitigation:**

- `ChannelManager.ensureChannelExists()` is already idempotent (returns existing channel if one exists)
- 409 response for explicit duplicate detection
- Database-level uniqueness constraint on (peerId, chain, token) for EVM channels

**Rollback Plan:**

1. Remove new route handlers from admin-api.ts
2. Redeploy — system reverts to internal-only channel management
3. BLS loses ability to manage channels; settlement reverts to automatic threshold-based flow

## Definition of Done

- [ ] All 5 stories completed with acceptance criteria met
- [ ] BLS can open, deposit, close, and query channels via Admin API
- [ ] BLS can query peer balances for pricing decisions
- [ ] Existing Admin API and automatic settlement flow unchanged
- [ ] Integration tests pass for channel lifecycle and balance queries
- [ ] Documentation updated (API docs, env vars)

## Related Work

- **Epic 17:** BTP Off-Chain Claim Exchange (claim infrastructure queried by Story 21.3)
- **Epic 19:** Production Deployment Parity (TigerBeetle accounting queried by Story 21.3)
- **Epic 20:** Bidirectional Agent-Runtime Middleware (`POST /ilp/send` and settlement on `POST /admin/peers`)
- **Agent-Society Epic 7:** SPSP Settlement Negotiation (primary consumer — uses channel open/verify during SPSP)
- **Agent-Society Epic 8:** Nostr Network Bootstrap (uses channel open during bootstrap, balance queries for pricing)
