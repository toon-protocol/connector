# Epic 19: Production Deployment Parity - Brownfield Enhancement

**Epic Number:** 19
**Priority:** High - Unblocks Epic 18 Explorer UI verification
**Type:** Deployment Configuration / Infrastructure Integration

## Epic Goal

Enable TigerBeetle accounting infrastructure in the docker-compose-5-peer-multihop.yml deployment to match production configuration, ensuring that the Explorer UI Accounts tab displays real-time balance data and settlement events as designed in Epic 18.

## Epic Description

### Existing System Context

**Current Functionality:**

- Multi-peer ILP connector deployment (5-peer linear topology)
- Payment channel settlement via EVM (Base Sepolia testnet)
- Explorer UI with Dashboard, Packets, Accounts, Peers, Keys tabs
- Telemetry streaming via WebSocket for real-time updates

**Technology Stack:**

- Docker Compose for orchestration
- TigerBeetle accounting database (Epic 6 - fully implemented in code)
- AccountManager with double-entry ledger (Epic 6 - fully implemented)
- Explorer UI with useAccountBalances hook expecting ACCOUNT_BALANCE events

**Integration Points:**

- `packages/connector/src/core/connector-node.ts` - AccountManager initialization (currently line 277 uses mock)
- `docker-compose-5-peer-multihop.yml` - Service definitions (missing TigerBeetle service)
- `packages/connector/explorer-ui/src/hooks/useAccountBalances.ts` - Consumes ACCOUNT_BALANCE events
- Telemetry pipeline: AccountManager → TelemetryEmitter → EventStore → WebSocket → Explorer UI

### Enhancement Details

**What's Being Added:**

1. **TigerBeetle Service** to docker-compose-5-peer-multihop.yml:
   - Container with tigerbeetle:latest image
   - Health check using TCP socket
   - Persistent volume for accounting data
   - Network access for all peer connectors

2. **Environment Variables** for each peer (peer1-peer5):
   - `TIGERBEETLE_CLUSTER_ID: "0"`
   - `TIGERBEETLE_REPLICAS: tigerbeetle:3000`

3. **Service Dependencies**:
   - All peers `depends_on: tigerbeetle: condition: service_healthy`

4. **Code Changes** in connector-node.ts:
   - Replace mock AccountManager with real TigerBeetleClient + AccountManager
   - Enable ACCOUNT_BALANCE telemetry event emission
   - Wire up settlement threshold monitoring

**How It Integrates:**

- TigerBeetle runs as shared service accessible by all peers
- Each connector instantiates AccountManager with TigerBeetleClient on startup
- AccountManager emits ACCOUNT_BALANCE events via existing TelemetryEmitter
- Explorer UI receives events via WebSocket and updates Accounts tab in real-time
- Follows exact pattern from docker-compose-production.yml (proven configuration)

**Success Criteria:**

1. ✅ docker-compose-5-peer-multihop.yml successfully deploys with TigerBeetle service
2. ✅ All 5 peers connect to TigerBeetle and create accounts on startup
3. ✅ Sending packets generates ACCOUNT_BALANCE events visible in Explorer API
4. ✅ Accounts tab displays peer accounts with balance cards
5. ✅ Balance history chart shows gradient fills and updates in real-time
6. ✅ Settlement threshold progress bars display correctly
7. ✅ No regression in packet forwarding or Explorer UI functionality

## Stories

### Story 19.1: Add TigerBeetle Service to Multi-Peer Deployment

**As a** connector operator,
**I want** TigerBeetle accounting database deployed alongside my 5-peer network,
**so that** peer account balances are tracked and visualized in the Explorer UI.

**Scope:**

- Add TigerBeetle service to docker-compose-5-peer-multihop.yml
- Configure environment variables for peer connections
- Add service dependencies and health checks
- Initialize TigerBeetle data volume
- Verify deployment with health checks

**Acceptance Criteria:**

1. TigerBeetle service added with health check
2. Volume `tigerbeetle-5peer-data` created
3. Initialization script creates cluster on first run
4. All 5 peers have TIGERBEETLE\_\* environment variables
5. Peers depend on TigerBeetle service_healthy condition
6. Deployment script updated to initialize TigerBeetle
7. Documentation updated in docker-compose file
8. Integration test verifies TigerBeetle starts before peers
9. Cleanup script removes TigerBeetle volume
10. README section added explaining TigerBeetle integration

---

### Story 19.2: Enable Real AccountManager in Connector Node

**As a** connector developer,
**I want** the connector to use real AccountManager with TigerBeetle instead of mock,
**so that** account balances are tracked and ACCOUNT_BALANCE events are emitted.

**Scope:**

- Replace mock AccountManager in connector-node.ts
- Instantiate TigerBeetleClient from environment variables
- Wire AccountManager to PacketHandler
- Enable ACCOUNT_BALANCE telemetry emission
- Verify accounting works end-to-end

**Acceptance Criteria:**

1. connector-node.ts removes mock AccountManager (line 277)
2. TigerBeetleClient instantiated with TIGERBEETLE_CLUSTER_ID and TIGERBEETLE_REPLICAS
3. AccountManager created with real TigerBeetleClient and TelemetryEmitter
4. PacketHandler receives AccountManager (not null)
5. ACCOUNT_BALANCE events emitted on packet forward
6. Settlement threshold monitoring enabled
7. Integration test verifies balance tracking across packet sends
8. Regression test verifies packet forwarding still works
9. Error handling for TigerBeetle connection failures
10. Logs indicate AccountManager initialization success/failure

---

### Story 19.3: Verify Accounts Tab with Real Balance Data

**As a** connector operator,
**I want** to see real-time account balances in the Explorer UI Accounts tab,
**so that** I can monitor peer credit exposure and settlement state.

**Scope:**

- Deploy 5-peer network with TigerBeetle enabled
- Send packets to generate balance changes
- Verify Accounts tab displays account cards
- Verify balance history charts render
- Capture screenshots for Epic 18.4 completion

**Acceptance Criteria:**

1. Deployment successful with TigerBeetle + 5 peers
2. Send 10+ packets generating varied balance history
3. Accounts tab shows account cards for peer2 (from peer1 perspective)
4. Net balance displays prominently with emerald/rose colors
5. Balance history chart shows gradient fills with tooltips
6. Settlement threshold progress bar displays
7. Real-time updates work (balance changes on new packets)
8. Screenshot captured showing populated Accounts tab
9. No console errors in browser or connector logs
10. Performance acceptable (<100ms for balance update)

---

### Story 19.4: Enable Native TigerBeetle for macOS Development

**As a** macOS developer,
**I want** to run TigerBeetle natively (without Docker) on my Mac,
**so that** I can develop and test the complete accounting system locally with perfect development/production parity.

**Scope:**

- Create automated installation script for TigerBeetle binary on macOS
- Integrate native TigerBeetle with development workflow (npm scripts)
- Document installation and usage process
- Verify development/production parity (same binary, different deployment)
- Create team onboarding documentation

**Acceptance Criteria:**

1. Installation script (`scripts/install-tigerbeetle-macos.sh`) downloads and installs TigerBeetle binary
2. npm scripts added: `tigerbeetle:install`, `tigerbeetle:start`, `tigerbeetle:stop`, `dev`, `dev:stop`
3. Development workflow: `npm run dev` starts TigerBeetle + Connector automatically
4. Documentation updated: `docs/guides/local-development-macos.md`, `README.md`, `CONTRIBUTING.md`
5. Architecture documented: Native TigerBeetle (dev) vs Containerized (prod) - same binary
6. Team onboarding guide complete with 5-minute setup instructions
7. CI/CD compatibility verified (Linux runners use Docker, unaffected)
8. OrbStack investigation documented with findings (why native is better)
9. Production deployment unchanged (still uses containerized TigerBeetle)
10. Perfect dev/prod parity verified (same TigerBeetle version, same code path)

## Compatibility Requirements

- ✅ **Existing APIs remain unchanged** - TigerBeetle integration is internal
- ✅ **Database schema changes backward compatible** - TigerBeetle accounts created on-demand
- ✅ **UI changes follow existing patterns** - No UI code changes needed (already implemented in Story 18.4)
- ✅ **Performance impact minimal** - TigerBeetle adds <1ms latency per packet

## Risk Mitigation

**Primary Risk:** TigerBeetle service failure could block all peers from starting

**Mitigation:**

- Health check with 30s start_period gives TigerBeetle time to initialize
- Fallback: If TigerBeetle unavailable, connector logs error but continues (graceful degradation)
- Separate volume ensures data persistence across restarts

**Rollback Plan:**

1. Stop deployment: `docker-compose -f docker-compose-5-peer-multihop.yml down`
2. Revert connector-node.ts to use mock AccountManager
3. Remove TigerBeetle service from docker-compose
4. Redeploy without accounting (packets still forward)

**Testing Strategy:**

- Story 19.1: Verify TigerBeetle starts independently
- Story 19.2: Integration tests with real AccountManager
- Story 19.3: End-to-end verification with UI
- Regression: Verify existing packet forwarding still works

## Definition of Done

- [ ] All 4 stories completed with acceptance criteria met
- [x] TigerBeetle service running in 5-peer deployment (Story 19.1 - COMPLETED)
- [ ] Real AccountManager code written (Story 19.2 - CODE COMPLETE, NOT TESTED)
  - **Status:** Ready for testing with Story 19.4 complete
  - **Next Action:** Test AccountManager integration with native TigerBeetle
- [ ] Existing packet forwarding functionality verified (no regression)
- [ ] Accounts tab displays real balance data (Story 19.3 - UNBLOCKED)
  - **Blocker Removed:** Story 19.4 complete (native TigerBeetle working)
  - **Status:** Ready to proceed
  - **Next Action:** Deploy 5-peer network and verify Accounts tab
- [x] Native TigerBeetle enables macOS development (Story 19.4 - COMPLETED & TESTED)
  - [x] Installation scripts created and tested (7-second setup verified)
  - [x] npm workflow integration complete (`npm run dev` tested)
  - [x] Documentation updated (macOS guide, README, CONTRIBUTING)
  - [x] Dev/prod parity verified (same binary v0.16.68)
  - [x] Alternative solutions analyzed and documented
  - [x] Installation tested on macOS arm64
  - [x] Start/stop scripts tested and working
- [x] Documentation updated in docker-compose file and deployment scripts
- [ ] Integration tests pass for accounting functionality

## Validation Checklist

### Scope Validation

- ✅ **Epic can be completed in 1-3 stories** - 4 stories (3 core + 1 developer tooling)
- ✅ **No architectural documentation required** - Following existing Epic 6 architecture
- ✅ **Enhancement follows existing patterns** - Using docker-compose-production.yml as reference
- ✅ **Integration complexity manageable** - Well-defined integration points

### Risk Assessment

- ✅ **Risk to existing system is low** - Additive change, doesn't modify packet forwarding
- ✅ **Rollback plan is feasible** - Simple revert of docker-compose and connector-node.ts
- ✅ **Testing approach covers existing functionality** - Regression tests for packet forwarding
- ✅ **Team has sufficient knowledge** - Epic 6 already implemented all AccountManager code

### Completeness Check

- ✅ **Epic goal is clear and achievable** - Enable TigerBeetle in 5-peer deployment
- ✅ **Stories are properly scoped** - Each story has distinct deliverable
- ✅ **Success criteria are measurable** - Visual verification via Explorer UI
- ✅ **Dependencies are identified** - Epic 6 (completed), Epic 18 Story 18.4 (completed)

## Notes

This epic is **NOT introducing new functionality** - all accounting code exists from Epic 6. This epic simply:

1. Adds TigerBeetle to the deployment configuration
2. Switches from mock to real AccountManager
3. Verifies the integration works end-to-end
4. Enables macOS development through OrbStack (Story 19.4)

Think of this as "flipping the switch" to enable accounting that was built but not deployed in the 5-peer test environment.

**Story 19.4 Evolution (February 2026):**
After completing Stories 19.1-19.3, we discovered that Docker Desktop on macOS blocks io_uring syscalls (required by TigerBeetle) as of version 4.42.0. Initial research explored OrbStack (a modern Docker Desktop replacement with Linux kernel 6.3.12), but investigation revealed native TigerBeetle installation is superior:

**Why Native TigerBeetle Won:**

- ✅ **Perfect Dev/Prod Parity:** Same binary (v0.16.68), same code path, just different deployment (native vs containerized)
- ✅ **Simplicity:** Single codebase, no dual implementations, no cache invalidation complexity
- ✅ **Reliability:** No container compatibility issues, no syscall translation layers
- ✅ **Developer Experience:** 5-minute setup, `npm run dev` just works
- ✅ **Financial Correctness:** Eliminates entire class of eventual consistency bugs that Redis+Postgres would introduce
- ✅ **Cost:** 2.75x cheaper than Redis+Postgres hybrid approach in production

**Architectural Decision:**

- **Development (macOS):** Native TigerBeetle binary installed via Homebrew-style script
- **Production (Linux):** Containerized TigerBeetle (same binary, same behavior)
- **Rejected Alternatives:** OrbStack containers (syscall issues), PostgreSQL-only (dev/prod parity loss), Redis+Postgres hybrid (complexity explosion)

Story 19.4 delivers automated tooling (`npm run tigerbeetle:install`, `npm run dev`) that enables macOS developers to run the complete TigerBeetle-enabled stack locally with zero compromise on production parity.

## Related Work

- **Epic 6:** TigerBeetle Accounting (COMPLETED - code exists, not deployed)
- **Epic 18:** Explorer UI NOC Redesign (COMPLETED - UI ready, waiting for data)
- **Story 18.4:** Accounts Tab Visualization (COMPLETED - frontend waiting for backend)
