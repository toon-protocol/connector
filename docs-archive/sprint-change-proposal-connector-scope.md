# Sprint Change Proposal: Scope Reduction to Connector + Explorer

**Date:** 2026-02-02
**Trigger:** User-initiated scope reduction
**Status:** Draft - Pending Approval

---

## 1. Issue Summary

### Change Trigger

The project scope is being intentionally reduced to focus exclusively on being an **ILP Connector implementation with Explorer UI**, removing all agent-specific, AI, Nostr, and advanced marketplace features.

### Rationale

- Simplify project focus to core connector functionality
- Remove complexity from agent orchestration, AI integration, and Nostr protocols
- Establish a clean, production-ready ILP connector with EVM (Base L2) settlement
- Maintain observability through the Explorer UI

### Current State

- 30 epics spanning connector, agent protocol, AI, zkVM, markets, streaming
- Significant code already removed on `epic-14` branch (per git status)
- README and docs still reflect full "M2M" vision with Agent Society Protocol

---

## 2. Epic Impact Summary

### Epics to RETAIN (15 total)

| Epic | Name                                | Status                    |
| ---- | ----------------------------------- | ------------------------- |
| 1    | Foundation & Core ILP Protocol      | Completed                 |
| 2    | BTP Protocol & Multi-Node Docker    | Completed                 |
| 3    | Real-Time Visualization Dashboard   | Completed                 |
| 4    | Logging, Configuration & DX         | Completed                 |
| 5    | Documentation and RFC Integration   | Completed                 |
| 6    | Settlement Foundation & Accounting  | Completed                 |
| 7    | Local Blockchain Dev Infrastructure | Completed                 |
| 8    | EVM Payment Channels (Base L2)      | Completed                 |
| 9    | ~~XRP Payment Channels~~            | Removed (EVM-only)        |
| 10   | CI/CD Pipeline Reliability          | Completed                 |
| 14   | Packet/Event Explorer UI            | Completed (needs cleanup) |
| 15   | Explorer Performance & UX           | Completed (needs cleanup) |
| 27   | ~~Aptos Payment Channels~~          | Removed (EVM-only)        |
| 28   | Public Testnet Integration          | Completed                 |
| 29   | Blockchain Explorer Links           | Completed                 |

### Epics to REMOVE (16 total)

| Epic | Name                                          | Reason                 |
| ---- | --------------------------------------------- | ---------------------- |
| 11   | AI Agent Wallet Infrastructure                | Agent-specific         |
| 12   | Multi-Chain Settlement & Production Hardening | Agent-focused scope    |
| 13   | Agent Society Protocol (ILP + Nostr)          | Core agent protocol    |
| 16   | AI Agent Node (Vercel AI SDK)                 | AI integration         |
| 17   | NIP-90 DVM Compatibility                      | Nostr protocol         |
| 18   | Agent Capability Discovery                    | Agent protocol         |
| 20   | Multi-Agent Coordination                      | Agent protocol         |
| 21   | Agent Reputation, Trust & Disputes            | Already marked removed |
| 22   | Emergent Workflow Composition                 | Agent protocol         |
| 23   | NIP-56XX Payment Streams                      | Agent protocol         |
| 24   | Live Streaming Infrastructure                 | Agent use case         |
| 25   | zkVM Compute Verification                     | Agent verification     |
| 26   | Agent Service Markets                         | Agent markets          |
| 30   | Balance Proof Exchange via Nostr              | Nostr-dependent        |

---

## 3. Artifact Adjustment Needs

### 3.1 Epic List (`docs/prd/epic-list.md`)

**Action:** Complete rewrite

**Proposed Content:**

```markdown
# Epic List

**Epic 1: Foundation & Core ILP Protocol Implementation**
Establish monorepo structure, implement RFC-0027 (ILPv4) packet handling and routing logic with OER encoding, and deliver basic packet forwarding capability with unit tests and logging.

**Epic 2: BTP Protocol & Multi-Node Docker Deployment**
Implement RFC-0023 BTP WebSocket communication between connectors, create Docker containerization with Compose orchestration, and enable deployment of configurable N-node networks with health checks.

**Epic 3: Real-Time Visualization Dashboard**
Build React-based network visualization showing topology and animated packet flow, implement telemetry aggregation from connector nodes, and provide interactive packet inspection capabilities.

**Epic 4: Logging, Configuration & Developer Experience**
Implement comprehensive structured logging with filterable log viewer, add support for multiple network topology configurations, create test packet sender utility, and complete documentation for user onboarding.

**Epic 5: Documentation and RFC Integration**
Create comprehensive developer documentation explaining ILP concepts and ensure all RFC references are accurate, accessible, and properly integrated into the project documentation.

**Epic 6: Settlement Foundation & Accounting**
Integrate TigerBeetle as the double-entry accounting database, build account management infrastructure to track balances and credit limits between peers, implement settlement threshold triggers, and provide dashboard visualization of account states and settlement events.

**Epic 7: Local Blockchain Development Infrastructure**
Establish local blockchain node infrastructure with Anvil (Base L2 fork) via Docker Compose, enabling developers to build and test payment channel smart contracts locally.

**Epic 8: EVM Payment Channels (Base L2)**
Implement payment channels as EVM smart contracts on Base L2, deploy payment channel infrastructure via Docker, integrate with settlement layer for automatic channel settlement.

**Epic 9: ~~XRP Payment Channels~~ (Removed - EVM-only)**

**Epic 10: CI/CD Pipeline Reliability & Test Quality**
Eliminate recurring CI/CD pipeline failures by fixing test quality issues, implementing pre-commit quality gates, and establishing systematic testing workflows.

**Epic 11: Packet Explorer UI**
Deliver a per-node web-based explorer interface embedded in each connector that visualizes packets and events in real-time. Provides block explorer-style inspection for ILP packets, settlements, and payment channels.

**Epic 12: Explorer Performance, UX & Visual Quality**
Polish the Explorer UI with performance optimizations (60fps at 1000+ events), UX improvements (keyboard shortcuts, responsive layout), and visual quality refinements.

**Epic 13: ~~Aptos Payment Channels~~ (Removed - EVM-only)**

**Epic 14: Public Testnet Integration**
Add `NETWORK_MODE=testnet/local` support for EVM (Base Sepolia), enabling integration tests against public testnets.

**Epic 15: Blockchain Explorer Navigation Links**
Transform static wallet addresses and transaction hashes into interactive, clickable links that open the corresponding blockchain explorer.

---

## Project Status

All 15 epics are **completed**. The connector is feature-complete with:

- RFC-compliant ILPv4 packet routing
- BTP WebSocket protocol for connector peering
- EVM settlement (Base L2)
- TigerBeetle accounting integration
- Explorer UI for real-time observability
- Public testnet support
```

---

### 3.2 README.md

**Action:** Major rewrite - focus on ILP Connector

**Key Changes:**

- Remove: Agent Society Protocol, Nostr, TOON, AI integration, Vercel AI SDK
- Remove: zkVM, prediction markets, live streaming references
- Rebrand: "M2M" → "ILP Connector" (or choose new name)
- Update architecture diagram: Remove agent nodes, facilitator, libSQL
- Update monorepo structure: Remove agent/, messaging/, workflow/ references
- Update technology stack: Remove nostr-tools, @vercel/ai, TOON
- Update project status: All epics completed

**Proposed README Structure:**

```
# ILP Connector

## TL;DR
A TypeScript implementation of an Interledger Protocol (ILP) connector with EVM (Base L2) settlement and real-time observability.

## Key Capabilities
- Multi-Hop Payment Routing (RFC-0027 ILPv4)
- BTP WebSocket Protocol (RFC-0023)
- EVM Settlement (Base L2)
- TigerBeetle Accounting
- Explorer UI for packet inspection

## Protocol Components
- Interledger Protocol (ILP) & BTP
- Payment Channels (EVM / Base L2)

## Architecture Overview
[Simplified diagram without agents]

## Installation & Usage
[Existing content, remove agent references]

## Documentation
[Remove agent-related links]
```

---

### 3.3 Epic 11 PRD (`docs/prd/epic-11-packet-event-explorer-ui.md`)

**Action:** Modify to remove agent-specific references

**Specific Edits:**

1. Line 5: Change

   > "agent activity"

   To:

   > "connector activity"

2. Line 47-49: Remove TOON Events bullet or change to:

   > "**Event Payloads**: Packet data with decoded content display"

3. Line 94: Change "TOON Content" to "Packet Data"

4. Line 275-280: Update detail panel description to remove TOON-specific rendering

5. Line 375-376: Remove dependencies on Epic 12 and Epic 13

---

### 3.4 Epic 12 PRD (`docs/prd/epic-12-agent-explorer-polish.md`)

**Action:** Rename and modify

**Specific Edits:**

1. Title: Change to "Epic 12: Connector Explorer — Performance, UX & Visual Quality"

2. Line 5: Change "Agent Explorer" → "Connector Explorer" throughout

3. Lines referencing "Agent Society test" → "Docker integration test"

4. Remove references to:
   - TOON payloads
   - Agent Society Protocol
   - Agent-specific telemetry

5. Update Story 15.1 title to "Rebrand to Connector Explorer & Docker Test Data Harness"

---

### 3.5 Architecture Documents

**Files to DELETE:**

- `docs/architecture/agent-society-protocol.md`
- `docs/architecture/ai-agent-skills.md`
- `docs/architecture/zkvm-verification-spec.md`

**Files to UPDATE:**

`docs/architecture/components.md`:

- Remove: AgentNode, AgentEventDatabase, AgentEventHandler, SubscriptionManager, FollowGraphRouter, ToonCodec, AIAgentDispatcher, SkillRegistry, TokenBudget
- Keep: ConnectorNode, PacketHandler, RoutingTable, BTPServer, BTPClient, BTPClientManager, OERCodec, TelemetryEmitter, UnifiedSettlementExecutor, EVMChannelLifecycleManager

`docs/architecture/data-models.md`:

- Remove Nostr event types, TOON format definitions
- Keep ILP packet types, settlement types, telemetry types

---

### 3.6 Epic PRD Files to DELETE

```
docs/prd/epic-11-ai-agent-wallet-infrastructure.md
docs/prd/epic-12-multi-chain-settlement-production-hardening.md
docs/prd/epic-13-agent-society-protocol.md
docs/prd/epic-16-ai-agent-node.md
docs/prd/epic-17-nip-90-dvm-compatibility.md
docs/prd/epic-18-agent-capability-discovery.md
docs/prd/epic-20-multi-agent-coordination.md
docs/prd/epic-21-agent-reputation-trust.md
docs/prd/epic-22-emergent-workflow-composition.md
docs/prd/epic-23-nip-56xx-payment-streams.md
docs/prd/epic-24-live-streaming-infrastructure.md
docs/prd/epic-25-zkvm-compute-verification.md
docs/prd/epic-26-agent-service-markets.md
docs/prd/epic-30-balance-proof-exchange.md
```

---

### 3.7 Code Changes (Already In Progress)

Per git status, the following code is already staged for removal on `epic-14`:

**Connector Package:**

- `src/agent/` - Agent server code (keep agent-server.ts shell if needed)
- `src/facilitator/` - Facilitator server, service registry, workflow handler
- `src/messaging/` - Giftwrap router, WebSocket server, messaging gateway
- `src/workflow/` - Image processor, workflow handler, peer server

**Explorer UI:**

- Messaging components (ContactSidebar, MessageBubble, MessageComposer, MessageList)
- EncryptionInspector, RoutingVisualization
- Private Messenger page
- Giftwrap hooks and types

**Shared Package:**

- `src/types/workflow.ts`

---

## 4. Recommended Path Forward

### Selected Path: Direct Adjustment / Integration

The scope reduction can be implemented through direct modifications to existing artifacts without requiring fundamental architectural changes. The core connector infrastructure remains intact.

### Rationale:

1. Core connector epics (1-10) are fully completed and unaffected
2. Settlement infrastructure (Epic 8, 28) remains intact
3. Explorer UI (Epics 14, 15, 29) needs only cosmetic changes (renaming, removing agent-specific features)
4. Code removal is already in progress on `epic-14` branch
5. No architectural rework required - just scope pruning

---

## 5. High-Level Action Plan

### Phase 1: Documentation Updates

1. [ ] Rewrite `docs/prd/epic-list.md` with 15 retained epics
2. [ ] Rewrite `README.md` to focus on ILP Connector
3. [ ] Update Epic 11 PRD (remove agent references)
4. [ ] Update Epic 12 PRD (rename to Connector Explorer)
5. [ ] Delete 14 removed epic PRD files
6. [ ] Delete/update architecture docs (agent-society, ai-skills, zkvm, components)

### Phase 2: Code Cleanup

1. [ ] Complete removal of agent/messaging/workflow code (in progress)
2. [ ] Update Explorer UI to remove agent-specific components
3. [ ] Rename "Agent Explorer" → "Connector Explorer" in UI
4. [ ] Remove TOON-specific rendering from Explorer
5. [ ] Update shared package to remove workflow types

### Phase 3: Verification

1. [ ] Verify all Docker tests pass with reduced scope
2. [ ] Verify Explorer UI renders correctly without agent features
3. [ ] Update CI/CD if needed for removed test files
4. [ ] Final documentation review

---

## 6. PRD MVP Impact

### Original MVP Scope

Full Agent Society Protocol with ILP + Nostr + AI + Markets

### New MVP Scope

**ILP Connector with EVM Settlement and Explorer UI**

- RFC-0027 ILPv4 packet routing
- RFC-0023 BTP WebSocket protocol
- EVM Payment Channels (Base L2)
- TigerBeetle accounting
- Explorer UI for observability
- Public testnet support

### Impact Assessment

- **Scope Reduction:** ~50% (15 of 30 epics retained)
- **Completed Features:** All retained epics are already completed
- **New Development:** None required - maintenance/cleanup only

---

## 7. Agent Handoff Plan

| Role          | Responsibility                 |
| ------------- | ------------------------------ |
| **Developer** | Execute Phase 2 (code cleanup) |
| **Developer** | Execute Phase 1 (doc updates)  |
| **QA**        | Verify Phase 3 (testing)       |

No additional PM/Architect involvement required - this is a scope reduction, not a redesign.

---

## 8. Success Criteria

- [ ] Epic list shows exactly 15 epics (all completed)
- [ ] README reflects connector-only focus
- [ ] No references to "Agent Society", "Nostr", "TOON", "AI Agent" in retained docs
- [ ] Explorer UI branded as "Connector Explorer" (or similar)
- [ ] All Docker tests pass
- [ ] CI/CD pipeline green

---

## 9. Rollback Plan

If issues arise:

1. Revert documentation changes via git
2. Restore deleted epic PRD files from git history
3. Code changes on `epic-14` can be reverted to `main`

---

**Approval Required:** User approval to proceed with implementation

**Estimated Effort:** Documentation updates (1-2 hours), Code verification (1 hour)
