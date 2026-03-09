# Epic 30: Per-Hop BLS Notification Pipeline

**Epic Number:** 30
**Priority:** High — Enables per-hop computation for agent networks without breaking ILP protocol compliance
**Type:** Brownfield Enhancement — Core Protocol Extension
**Dependencies:** Epic 24 (Connector Library API — `LocalDeliveryClient`, `setLocalDeliveryHandler()`), Epic 22 (Agent-Runtime Simplification — `SHA256(data)` fulfillment model)

## Epic Goal

Enable every connector in an ILP packet's path to notify its local Business Logic Server (BLS) via a non-blocking fire-and-forget HTTP POST, while preserving the existing blocking delivery at the final hop where the BLS decides accept/reject. This transforms the ILP routing path into a computation pipeline where each hop can observe, log, or trigger side-effects based on transiting packets.

## Epic Description

### Existing System Context

**Current Functionality:**

- `PacketHandler` in `packet-handler.ts` (lines 948-1026) routes packets using a binary decision: if `nextHop === this.nodeId` or `nextHop === 'local'`, deliver locally via `LocalDeliveryClient.deliver()` (blocking, awaits BLS response); otherwise forward to the next peer via BTP (no BLS notification)
- `LocalDeliveryClient.deliver()` (in `local-delivery-client.ts`) sends an HTTP POST to `{handlerUrl}/handle-packet` with a `PaymentRequest` payload (`paymentId`, `destination`, `amount`, `expiresAt`, `data`) and returns an ILP FULFILL or REJECT based on the BLS response
- Intermediate connectors have zero visibility into packet contents at the application layer — they validate, adjust amount/expiry, and forward via BTP only
- The `LocalDeliveryConfig` (`config/types.ts` line 1694) controls whether local delivery is enabled, the handler URL, timeout, and auth token
- The in-process `localDeliveryHandler` function (set via `setLocalDeliveryHandler()`) is checked before the HTTP client — this is for library-mode consumers

**Technology Stack:**

- TypeScript 5.3.3, Node.js 22 LTS, npm workspaces monorepo
- `LocalDeliveryClient` uses native `fetch()` with `AbortController` timeout
- `PaymentRequest`/`PaymentResponse` types in `payment-handler.ts`
- BTP over WebSocket for inter-connector forwarding

**Integration Points:**

- `packages/connector/src/core/packet-handler.ts` — Routing decision logic (lines 948-1026 for local, lines 1028+ for forwarding)
- `packages/connector/src/core/local-delivery-client.ts` — HTTP POST to `/handle-packet`
- `packages/connector/src/core/payment-handler.ts` — `PaymentRequest`, `PaymentResponse` types
- `packages/connector/src/config/types.ts` — `LocalDeliveryConfig`, `ConnectorConfig`
- `packages/connector/src/config/config-loader.ts` — Config validation and env var loading

### Enhancement Details

**What's Being Changed:**

1. **New `perHopNotification` config field** on `LocalDeliveryConfig` (default: `false`) — when enabled, intermediate hops fire a non-blocking POST to the BLS in addition to forwarding via BTP
2. **Fire-and-forget notification in PacketHandler** — after routing table lookup determines `nextHop` is a peer (not local), if `perHopNotification` is enabled and `LocalDeliveryClient` is configured, call `deliver()` without `await` (`.catch(noop)` for error suppression)
3. **Optional `isTransit` flag on `PaymentRequest`** — allows the BLS to distinguish between final-hop delivery (where it must respond with accept/reject) and transit notification (where its response is ignored)
4. **In-process handler support** — when `localDeliveryHandler` is set (library mode), fire-and-forget calls the handler without `await` for transit packets, maintaining parity with HTTP mode
5. **Telemetry emission** — emit a new `PER_HOP_NOTIFICATION` telemetry event for observability of fire-and-forget delivery attempts

**How It Integrates:**

- The existing local delivery path (final hop) is completely unchanged — `await localDeliveryClient.deliver()` continues to block and return FULFILL/REJECT as today
- The new fire-and-forget path is additive — it runs in parallel with the existing BTP forwarding and cannot affect the critical path
- The same `PaymentRequest` payload format is used for both modes — the BLS receives the same data regardless of hop position
- Configuration is backward compatible — `perHopNotification` defaults to `false`, so existing deployments are unaffected
- No ILP protocol violation — the packet is forwarded unchanged to the next hop; the notification is a pure side-effect

**Success Criteria:**

1. When `perHopNotification: true`, every intermediate connector POSTs to its BLS when a packet transits through, without blocking the forwarding path
2. Final-hop delivery behavior is completely unchanged — BLS response determines FULFILL/REJECT
3. A failed fire-and-forget POST does not affect packet forwarding (no error propagation, no latency impact)
4. The BLS receives the same `PaymentRequest` format at both transit and final-hop delivery, with an `isTransit` flag to differentiate
5. All existing tests pass without modification (backward compatibility)
6. Measurable: fire-and-forget adds <1ms to the per-hop forwarding latency (serialization + dispatch only, no `await`)

## Stories

### Story 30.1: Add Per-Hop Notification Config and PaymentRequest Extension

**Description:** Extend `LocalDeliveryConfig` with the `perHopNotification` boolean field and add the optional `isTransit` flag to `PaymentRequest`. Update `ConfigLoader` validation and env var support. Types and validation only — no behavioral changes.

**Acceptance Criteria:**

- `LocalDeliveryConfig` extended with `perHopNotification?: boolean` (default: `false`)
- Environment variable support: `LOCAL_DELIVERY_PER_HOP_NOTIFICATION` (default: `'false'`)
- `PaymentRequest` interface extended with optional `isTransit?: boolean` field
- `ConfigLoader.validateConfig()` passes through the new field
- New types exported from `lib.ts`
- TypeScript compilation succeeds, all existing tests pass unchanged
- JSDoc documentation on new fields explains the fire-and-forget semantics

### Story 30.2: Implement Fire-and-Forget BLS Notification in PacketHandler

**Description:** Modify `PacketHandler` to fire a non-blocking BLS notification at intermediate hops when `perHopNotification` is enabled. The notification uses the existing `LocalDeliveryClient.deliver()` or `localDeliveryHandler` without `await`. The critical forwarding path via BTP remains completely unchanged.

**Acceptance Criteria:**

- After routing table lookup resolves to a peer (not local), if `perHopNotification` is enabled and local delivery is configured:
  - HTTP mode: `this.localDeliveryClient.deliver(packet, sourcePeerId).catch(noop)` — no `await`
  - In-process mode: `this.localDeliveryHandler(request, sourcePeerId).catch(noop)` — no `await`
- The `PaymentRequest` sent includes `isTransit: true` for intermediate hops, `isTransit: false` (or omitted) for final-hop delivery
- The BTP forwarding path (`forwardToNextHop`) is called unconditionally — the fire-and-forget notification runs in parallel
- Failed notifications are logged at `debug` level (not `warn` or `error`) to avoid noisy logs
- A `noop` error handler prevents unhandled promise rejections
- Existing final-hop delivery behavior is completely unchanged
- Unit tests verify: (a) notification fires when enabled, (b) notification does NOT fire when disabled, (c) forwarding succeeds even when notification throws, (d) `isTransit` flag is set correctly

### Story 30.3: Telemetry and Integration Test

**Description:** Add telemetry emission for per-hop notifications and create an integration test proving the full pipeline works across a multi-hop topology.

**Acceptance Criteria:**

- New telemetry event `PER_HOP_NOTIFICATION` emitted when a fire-and-forget notification is dispatched (includes `destination`, `amount`, `nextHop`, `correlationId`)
- Integration test with 3-connector chain (A → B → C):
  - Connector B has `perHopNotification: true` and a mock BLS
  - Connector C is the final hop with a BLS that returns `{ accept: true }`
  - Packet sent from A to C arrives and is fulfilled
  - Mock BLS at B receives the transit notification with `isTransit: true`
  - Mock BLS at C receives the final delivery with `isTransit: false` (or omitted) and returns accept
  - Latency measurement confirms B's fire-and-forget adds negligible overhead (<5ms tolerance)
- Explorer UI telemetry tab displays `PER_HOP_NOTIFICATION` events (if Explorer UI is running)

### Story 30.4: Remove XRP/Ripple and Aptos Settlement Support

**Description:** Delete all XRP/XRPL and Aptos settlement code, tests, types, and npm dependencies. Simplify `UnifiedSettlementExecutor`, `ConnectorNode`, config types, BTP claim types, and telemetry to EVM-only. ~41 files deleted, ~29 files modified.

**Acceptance Criteria:**

- All XRP/XRPL source files and tests deleted (xrpl-client, xrp-channel-sdk/manager/lifecycle, xrp-claim-signer, xrp-wss-connection-pool)
- All Aptos source files and tests deleted (aptos-channel-sdk, aptos-client, aptos-claim-signer, aptos-env-validator)
- `UnifiedSettlementExecutor` simplified to EVM-only
- `ConnectorNode` Aptos initialization removed
- Config types, BTP claim types, settlement types, telemetry, CLI, and explorer cleaned of XRP/Aptos references
- XRP/Aptos npm dependencies removed
- TypeScript compiles cleanly, remaining EVM-only tests pass

### Story 30.5: Adopt Crosstown Anvil + Faucet Test Infrastructure

**Description:** Copy Crosstown's Express.js faucet to `packages/faucet/`, create `docker-compose-evm-test.yml` with clean-chain Anvil (chain-id 31337, no Base Sepolia fork), replace shell-based faucet. Enables fully offline EVM testing.

**Acceptance Criteria:**

- Express.js faucet at `packages/faucet/` with JSON API (fund, info, health)
- `docker-compose-evm-test.yml` with clean-chain Anvil + embedded contract deployment + faucet
- Old docker-compose files updated or replaced
- `scripts/run-evm-tests.sh` created
- All EVM tests pass against new infrastructure, no external RPC required

### Story 30.6: Aggressive Test Suite Cleanup — Embedded-Mode EVM Focus

**Description:** Reclassify 8 misplaced unit tests out of `test/integration/`, delete fake acceptance tests, delete Docker orchestration and standalone-mode tests, consolidate redundant Base payment channel tests, remove performance tests from CI, right-size timeouts, create `waitFor()` helper.

**Acceptance Criteria:**

- 8 misclassified tests moved to `test/unit/`
- Fake acceptance tests deleted (production-acceptance, performance-benchmark-acceptance, load-test-24h)
- Docker orchestration tests deleted (6 files), standalone admin API tests deleted (2 files)
- 3 Base payment channel tests consolidated into 1
- Performance tests removed from Jest CI
- Timeouts right-sized, `waitFor()` helper created
- Jest config and package.json test scripts cleaned up

### Story 30.7: Embedded-Mode EVM Integration Test with Anvil

**Description:** Create a comprehensive embedded-mode integration test that exercises the full EVM payment channel lifecycle (open, fund, route packets, claim exchange, close, on-chain settlement) against a local Anvil node, with per-hop notification verification.

**Acceptance Criteria:**

- New `test/integration/embedded-evm-settlement.test.ts`
- 3 embedded ConnectorNode instances with `setLocalDeliveryHandler()` BLS mocks
- Full payment channel lifecycle tested against real Anvil blockchain state
- Per-hop notification and `PER_HOP_NOTIFICATION` telemetry verified in settlement flow
- On-chain state verified via ethers.js (balances, channel state)
- Gated behind `EVM_INTEGRATION=true`, completes under 60s

## Compatibility Requirements

- [x] Existing APIs remain unchanged — `LocalDeliveryConfig` addition is an optional field with `false` default
- [x] Database schema changes are backward compatible — N/A (no schema changes)
- [x] UI changes follow existing patterns — N/A (telemetry event is additive only)
- [x] Performance impact is minimal — fire-and-forget adds only serialization + HTTP dispatch overhead (~<1ms), no blocking
- [x] All existing YAML configs work without modification — `perHopNotification` defaults to `false`
- [x] ILP protocol compliance maintained — packet forwarded unchanged, notification is a pure side-effect

## Risk Mitigation

- **Primary Risk:** Fire-and-forget HTTP calls accumulating under high packet throughput, creating back-pressure on the BLS or exhausting HTTP connections
- **Mitigation:** The notification uses the same `fetch()` with `AbortController` timeout as final-hop delivery (default 30s). Under extreme load, timed-out notifications are silently dropped. Future enhancement could add rate limiting or circuit-breaker logic, but for MVP the timeout + catch(noop) pattern is sufficient.
- **Secondary Risk:** Unhandled promise rejections from fire-and-forget calls
- **Mitigation:** All fire-and-forget paths use `.catch(noop)` to suppress errors. Debug-level logging captures failures for investigation without alerting.
- **Rollback Plan:** Set `perHopNotification: false` (or remove the config field) to disable completely. The feature is fully gated behind a config flag — disabling returns to exact current behavior with zero code changes.

## Definition of Done

- [x] Stories 30.1-30.3 completed: per-hop notification config, fire-and-forget implementation, telemetry emission
- [ ] Story 30.4 completed: XRP/Ripple and Aptos code fully removed, EVM-only codebase
- [ ] Story 30.5 completed: Crosstown Anvil + faucet infrastructure adopted, offline EVM testing enabled
- [ ] Story 30.6 completed: Test suite aggressively cleaned — misclassified tests moved, fake tests deleted, redundant tests consolidated, embedded-mode focus
- [ ] Story 30.7 completed: Embedded-mode EVM integration test proves full payment channel lifecycle + per-hop notification against real Anvil blockchain
- [ ] TypeScript compiles cleanly across all packages
- [ ] `npm test` passes with zero failures
- [ ] Embedded-mode EVM integration test passes against local Anvil
- [ ] No XRP/Aptos code remains in source tree
- [ ] Documentation updated to reflect EVM-only focus
