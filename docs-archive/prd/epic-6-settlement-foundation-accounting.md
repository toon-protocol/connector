# Epic 6: Settlement Foundation & Accounting

**Goal:** Establish the foundational settlement layer for the M2M economy by integrating TigerBeetle as the double-entry accounting database and building account management infrastructure to track balances, credit limits, and settlement obligations between ILP connector peers. This epic delivers a production-ready accounting system that accurately tracks value transfer through ILP packet flows, enforces credit limits, triggers settlement events when thresholds are exceeded, and integrates with the existing telemetry system to visualize account balances and settlement activity in the dashboard. This is the first of four epics that will build complete cryptocurrency payment channel settlement infrastructure for AI agents and M2M micropayments.

## Story 6.1: TigerBeetle Integration & Docker Deployment

As a connector operator,
I want TigerBeetle deployed as a containerized service alongside my connector nodes,
so that I have a reliable, high-performance accounting database for tracking settlement balances.

### Acceptance Criteria

1. TigerBeetle added to `docker-compose.yml` as a new service with persistent volume for data directory
2. TigerBeetle configured with appropriate cluster settings (single-node for development, multi-replica configuration documented for production)
3. TigerBeetle service exposes port for client connections from connector containers
4. Health check implemented for TigerBeetle service (verify cluster is ready before connectors start)
5. Environment variables support configurable TigerBeetle cluster ID and replica count
6. Docker Compose `depends_on` ensures TigerBeetle starts before connector nodes
7. TigerBeetle data persists across container restarts using Docker volumes
8. Documentation added to `docs/guides/tigerbeetle-deployment.md` explaining cluster setup and operational considerations
9. README updated with TigerBeetle service information and connection details
10. Integration test verifies TigerBeetle container starts successfully and accepts client connections

---

## Story 6.2: TigerBeetle Client Library Integration

As a connector developer,
I want a TypeScript client wrapper for TigerBeetle operations,
so that I can interact with the accounting database using type-safe APIs.

### Acceptance Criteria

1. `tigerbeetle-node` npm package added as dependency to `packages/connector/package.json`
2. `TigerBeetleClient` class implemented in `packages/connector/src/settlement/tigerbeetle-client.ts` wrapping the native client
3. Client initialization accepts cluster ID and replica addresses from environment variables
4. Client exposes methods for creating accounts, creating transfers, and querying balances
5. Client implements connection pooling and automatic reconnection on failures
6. Client gracefully handles TigerBeetle errors and maps them to application-level error types
7. Client logs all TigerBeetle operations (account creation, transfers) with structured logging
8. Client implements timeout handling for operations (configurable timeout, default 5 seconds)
9. Unit tests verify client initialization, error handling, and operation retry logic using mocked TigerBeetle responses
10. Integration test connects to real TigerBeetle container and performs basic operations (create account, transfer)

---

## Story 6.3: Account Management and Double-Entry Ledger

As a connector node,
I want to manage double-entry accounts for each peer connection,
so that I can accurately track balances for both directions of value transfer.

### Acceptance Criteria

1. `AccountManager` class implemented in `packages/connector/src/settlement/account-manager.ts`
2. Each peer connection has two TigerBeetle accounts created (debit account, credit account for duplex channel model)
3. Account creation includes metadata: peer ID, account type (debit/credit), currency/token identifier
4. `AccountManager.createPeerAccounts(peerId, tokenId)` method creates both accounts atomically
5. Account IDs follow deterministic generation scheme: `hash(nodeId, peerId, tokenId, direction)` for idempotency
6. Account manager validates account creation success and handles duplicate account creation gracefully
7. Account manager exposes `getAccountBalance(peerId, tokenId)` method to query current balances
8. Account manager maintains in-memory cache of account IDs for fast lookups during packet forwarding
9. Unit tests verify account creation, balance queries, and cache invalidation logic
10. Integration test creates accounts for 10 peers and verifies balances are correctly initialized to zero

---

## Story 6.4: Settlement Event Recording via Packet Handler Integration

As a packet handler,
I want to record double-entry transfers in TigerBeetle for every ILP packet forwarded,
so that account balances accurately reflect value transferred through the network.

### Acceptance Criteria

1. `PacketHandler.handlePreparePacket()` integrates with `AccountManager` to record transfers
2. When receiving ILP Prepare packet from peer A, credit account for peer A is debited by packet amount
3. When forwarding packet to peer B, debit account for peer B is credited by packet amount (minus connector fee)
4. Transfers include metadata: packet ID (execution condition hash), timestamp, peer IDs
5. Transfers are posted to TigerBeetle atomically (both legs of double-entry or neither)
6. Failed transfers (TigerBeetle unavailable) trigger ILP Reject with `T00_INTERNAL_ERROR` code
7. Packet handler logs all settlement events (account debits/credits) with correlation IDs matching packet flow logs
8. Connector fee calculation implemented and deducted from forwarded amount (configurable fee percentage, default 0.1%)
9. Unit tests verify transfer recording for various packet scenarios (successful forward, rejected packet, timeout)
10. Integration test sends 100 packets through 3-node network and verifies TigerBeetle balances match expected values

---

## Story 6.5: Credit Limits and Balance Enforcement

As a connector operator,
I want to configure credit limits per peer and reject packets that would exceed those limits,
so that I can manage counterparty risk and prevent excessive exposure.

### Acceptance Criteria

1. Credit limit configuration added to peer config schema in `packages/connector/src/config/types.ts`
2. Credit limit stored per peer in `AccountManager` (default: unlimited for backward compatibility)
3. `AccountManager.checkCreditLimit(peerId, amount)` method validates if transfer would exceed limit
4. Packet handler calls credit limit check before recording transfer to TigerBeetle
5. Packets rejected with `T04_INSUFFICIENT_LIQUIDITY` error code if credit limit would be exceeded
6. Credit limit violations logged as warnings with peer ID, current balance, limit, and requested amount
7. Credit limit configuration supports token-specific limits (e.g., different limits for different ERC20 tokens)
8. Environment variable override for global credit limit ceiling (security safety valve)
9. Unit tests verify credit limit enforcement for various scenarios (at limit, over limit, under limit)
10. Integration test demonstrates credit limit rejection by sending packets until limit reached and verifying ILP Reject response

---

## Story 6.6: Settlement Threshold Detection and Triggers

As a connector node,
I want to detect when peer balances exceed configured settlement thresholds,
so that I can trigger settlement events to reconcile outstanding balances.

### Acceptance Criteria

1. Settlement threshold configuration added to peer config schema (default: 1000 units)
2. `SettlementMonitor` class implemented in `packages/connector/src/settlement/settlement-monitor.ts`
3. Settlement monitor polls account balances periodically (configurable interval, default 30 seconds)
4. When balance exceeds threshold, settlement monitor emits `SETTLEMENT_REQUIRED` event with peer ID, balance, and threshold
5. Settlement monitor tracks settlement state per peer (IDLE, SETTLEMENT_PENDING, SETTLEMENT_IN_PROGRESS)
6. Settlement monitor prevents duplicate settlement triggers (only trigger once per threshold crossing)
7. Settlement monitor logs all threshold crossings and settlement triggers with structured logging
8. Settlement monitor integrates with telemetry emitter to send `SETTLEMENT_TRIGGERED` events to dashboard
9. Unit tests verify threshold detection logic and settlement trigger emission
10. Integration test configures low threshold (100 units), forwards packets to exceed threshold, and verifies settlement trigger event

---

## Story 6.7: Settlement API Stub and Mock Settlement Execution

As a settlement engine,
I want a stub HTTP API endpoint for triggering settlement and recording settlement completions,
so that external settlement processes can notify the connector when settlements complete.

### Acceptance Criteria

1. `SettlementAPI` class implemented in `packages/connector/src/settlement/settlement-api.ts` using Express
2. Settlement API exposes `POST /settlement/execute` endpoint accepting peer ID and amount
3. Settlement API endpoint validates request format and returns 400 for malformed requests
4. Settlement API endpoint triggers mock settlement execution (logs "Settlement executed" and returns success)
5. Mock settlement updates TigerBeetle accounts to reflect settlement (reduces outstanding balance)
6. Settlement API exposes `GET /settlement/status/:peerId` endpoint returning current balance and settlement state
7. Settlement API integrated with connector's HTTP health server (shares port, default 8080)
8. Settlement API implements authentication using bearer token from environment variable
9. Unit tests verify API request validation, authentication, and response formats
10. Integration test calls settlement API to execute mock settlement and verifies balance reduction in TigerBeetle

---

## Story 6.8: Dashboard Telemetry Integration for Settlement Visualization

As a dashboard user,
I want to see account balances, credit limits, and settlement events in the network visualization,
so that I can monitor the financial health of connector peers in real-time.

### Acceptance Criteria

1. `ACCOUNT_BALANCE` telemetry event type added to `packages/shared/src/types/telemetry.ts`
2. Telemetry emitter sends account balance updates whenever balances change (on packet forward, settlement)
3. Account balance telemetry includes: peer ID, current balance, credit limit, settlement threshold, timestamp
4. `SETTLEMENT_EVENT` telemetry type added for settlement triggers and completions
5. Settlement event telemetry includes: peer ID, settlement amount, trigger reason, status (pending/complete)
6. Dashboard backend (`packages/dashboard`) handles new telemetry event types and stores recent balances in memory
7. Dashboard frontend displays balance information in network graph (balance badge on peer connections)
8. Dashboard frontend shows settlement events in timeline view with animated indicators
9. Dashboard frontend includes "Settlement Status" panel showing all peers with balances and settlement states
10. Integration test verifies telemetry events flow from connector to dashboard and are displayed correctly

---

## Epic Completion Criteria

- [ ] TigerBeetle successfully deployed via Docker Compose alongside connector nodes
- [ ] All connector-to-peer packet forwards record double-entry transfers in TigerBeetle
- [ ] Account balances visible in dashboard with real-time updates
- [ ] Credit limits enforced and packets rejected when limits exceeded
- [ ] Settlement thresholds trigger settlement events with telemetry notifications
- [ ] Mock settlement API functional and integrated with TigerBeetle
- [ ] All unit tests passing with >85% code coverage for settlement package
- [ ] Integration tests verify end-to-end accounting flow across 3-node network
- [ ] Documentation complete for TigerBeetle deployment and settlement configuration
- [ ] Performance validated: 1000 packets/second with TigerBeetle recording all transfers

---

## Dependencies and Integration Points

**Depends On:**

- Epic 1: Core ILP packet handling and routing table
- Epic 2: BTP protocol and multi-node deployment
- Epic 3: Dashboard telemetry infrastructure

**Integrates With:**

- `PacketHandler` - Records transfers on packet forward
- `BTPClient/BTPServer` - Peer connection management for account creation
- `TelemetryEmitter` - Sends balance and settlement events to dashboard
- `ConnectorNode` - Configuration loading for credit limits and thresholds

**Enables:**

- Epic 7: EVM Payment Channels (Base L2) - Settlement execution via smart contracts
- Epic 8: XRP Payment Channels - Settlement execution via XRP Ledger
- Epic 9: Multi-Chain Settlement - Cross-chain settlement coordination

---

## Technical Architecture Notes

### TigerBeetle Account Model

Each peer connection requires two TigerBeetle accounts for duplex channel:

```
Connector Node A ↔ Peer B

Account 1 (A owes B):
  - Ledger: hash(A, B, "outbound")
  - Code: token ID (e.g., USDC on Base)
  - Debit: Amount A has sent to B
  - Credit: Settlements from A to B

Account 2 (B owes A):
  - Ledger: hash(A, B, "inbound")
  - Code: token ID
  - Debit: Amount B has sent to A
  - Credit: Settlements from B to A
```

### Settlement Threshold Logic

```typescript
// Threshold detection
const outboundBalance = await getBalance(accountOutbound);
const settlementThreshold = peerConfig.settlementThreshold;

if (outboundBalance > settlementThreshold) {
  settlementMonitor.triggerSettlement(peerId, outboundBalance);
}
```

### Future Settlement Execution (Deferred to Epics 7-8)

This epic implements the **accounting foundation**. Actual cryptocurrency settlement via:

- Epic 7: EVM payment channels on Base L2
- Epic 8: XRP Ledger payment channels

Will replace the mock settlement API with real blockchain transactions.

---

## Testing Strategy

**Unit Tests:**

- TigerBeetle client wrapper error handling
- Account manager balance calculations
- Credit limit validation logic
- Settlement threshold detection

**Integration Tests:**

- TigerBeetle container deployment
- Multi-node packet flow with balance updates
- Credit limit enforcement across network
- Settlement trigger and mock execution

**Performance Tests:**

- 1000 packets/second with TigerBeetle recording
- Balance query latency (<10ms p99)
- Settlement threshold detection latency

---

## Security Considerations

1. **TigerBeetle Access Control:** TigerBeetle client connections restricted to connector containers only (Docker network isolation)
2. **Settlement API Authentication:** Bearer token required for settlement API endpoints to prevent unauthorized settlement triggers
3. **Credit Limit Safety Valve:** Global credit limit ceiling prevents misconfiguration from creating unbounded exposure
4. **Double-Entry Integrity:** TigerBeetle atomic transfers ensure accounting consistency (both legs or neither)
5. **Audit Trail:** All transfers include metadata for reconciliation and forensic analysis

---

## Documentation Deliverables

1. `docs/guides/tigerbeetle-deployment.md` - TigerBeetle setup and operational guide
2. `docs/guides/settlement-configuration.md` - Credit limits, thresholds, and settlement API usage
3. `docs/architecture/settlement-layer.md` - Settlement architecture and double-entry accounting model
4. API documentation for settlement HTTP endpoints
5. Telemetry event schema documentation for balance and settlement events

---

## Success Metrics

- TigerBeetle deployment success rate: 100% (container starts on `docker-compose up`)
- Accounting accuracy: 100% (balances match expected values after packet flow)
- Credit limit enforcement: 100% (no packets accepted over limit)
- Settlement trigger accuracy: 100% (all threshold crossings detected within 30 seconds)
- Performance: 1000 packets/second sustained with TigerBeetle recording
- Dashboard balance update latency: <500ms from packet to visualization
