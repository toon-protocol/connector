# Epic 11: Packet/Event Explorer UI

## Executive Summary

Deliver a per-node web-based explorer interface embedded in each connector that visualizes packets and events flowing through the network in real-time. The explorer provides block explorer-style inspection capabilities for ILP packets, settlements, and payment channel activity, with full event persistence via libSQL for historical browsing and analysis.

## Vision

Create a comprehensive observability layer that enables:

- **Real-time event streaming** from the local connector node
- **Block explorer-style navigation** of ILP packets, settlements, and payment channels
- **Deep packet inspection** with OER field decoding
- **Historical event browsing** with search, filtering, and pagination
- **Account and settlement tracking** for balance monitoring
- **Educational debugging** supporting the project's learning-first philosophy

## Architecture Decision: Embedded Per-Node Explorer

### Key Insight

Each connector node serves its own explorer UI on a dedicated port (default: 3001), providing a self-contained observability interface without external dependencies:

- Explorer is part of the connector deployment (single Docker image)
- Each node shows only its own perspective (sent/received events)
- No centralized dashboard aggregation complexity
- Aligns with M2M's educational approach (see one node's view clearly)

### Benefits

| Alternative Approach            | Embedded Explorer Approach        |
| ------------------------------- | --------------------------------- |
| Separate dashboard deployment   | Single deployment unit per node   |
| Network-wide aggregation        | Node-local perspective (clear)    |
| Complex multi-node coordination | Independent per-node operation    |
| Additional infrastructure       | Zero additional services required |

## Core Features

### 1. Real-Time Event Stream

Live WebSocket connection from browser to connector displaying:

- **ILP Packets**: Prepare, Fulfill, Reject with routing metadata
- **Settlement Events**: Account balances, threshold triggers, completions
- **Payment Channel Events**: Opens, updates, settlements, closures
- **Security Events**: Rate limits, suspicious activity, wallet mismatches

### 2. Event Persistence

libSQL database for each connector storing all telemetry events:

```sql
CREATE TABLE events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  event_type TEXT NOT NULL,          -- Telemetry event type
  timestamp INTEGER NOT NULL,         -- Unix timestamp (ms)
  node_id TEXT NOT NULL,              -- Connector node ID
  direction TEXT,                     -- 'sent' | 'received' | 'internal'
  peer_id TEXT,                       -- Related peer if applicable
  packet_id TEXT,                     -- ILP packet ID if applicable
  amount TEXT,                        -- Amount (as string for BigInt)
  destination TEXT,                   -- ILP destination address
  payload TEXT NOT NULL               -- Full event JSON
);

CREATE INDEX idx_events_type ON events(event_type);
CREATE INDEX idx_events_timestamp ON events(timestamp DESC);
CREATE INDEX idx_events_packet ON events(packet_id);
CREATE INDEX idx_events_peer ON events(peer_id);
```

### 3. Event Table View

Searchable, filterable table with columns:

| Column      | Description                         |
| ----------- | ----------------------------------- |
| Time        | Relative timestamp (e.g., "2s ago") |
| Type        | Event type badge (color-coded)      |
| Direction   | → Sent / ← Received / ⟳ Internal    |
| Peer        | Related peer node ID                |
| Destination | ILP address (for packets)           |
| Amount      | Value transferred (for payments)    |
| Status      | Success/Failure indicator           |

### 4. Event Detail Panel

Expandable detail view showing:

- **Raw Event JSON**: Formatted, syntax-highlighted
- **ILP Packet Fields**: Decoded OER fields with explanations
- **Related Events**: Links to corresponding Fulfill/Reject for Prepares
- **Timing**: Full timestamp, latency if applicable

### 5. Filters and Search

- **Event Type Filter**: Multi-select by event category
- **Time Range**: Quick selectors (1m, 5m, 1h, 24h) + custom range
- **Text Search**: Search across destination, peer, packet ID
- **Direction Filter**: Sent only, received only, all
- **Amount Filter**: Greater than / less than threshold

## Technology Stack

### Frontend

- **React 18.3.x** - Component framework
- **Vite 5.x** - Build tool with HMR
- **shadcn/ui v4** - UI component library (Radix + Tailwind)
- **TailwindCSS 3.4.x** - Utility-first styling
- **TypeScript 5.3.x** - Type safety

### Backend (Embedded in Connector)

- **Express.js** - Static file serving for UI bundle
- **WebSocket (ws)** - Real-time event streaming to browser
- **libSQL** - Event persistence
- **Shared Types** - Telemetry types from `packages/shared`

### Package Structure

```
packages/connector/
├── src/
│   ├── explorer/
│   │   ├── index.ts              # Explorer server initialization
│   │   ├── explorer-server.ts    # Express + WebSocket server
│   │   ├── event-store.ts        # libSQL event persistence
│   │   ├── event-broadcaster.ts  # WebSocket broadcast to browsers
│   │   └── routes/
│   │       ├── events.ts         # REST API for historical queries
│   │       └── health.ts         # Health check endpoint
│   └── ...existing connector code...
├── explorer-ui/                   # Frontend source
│   ├── src/
│   │   ├── App.tsx
│   │   ├── components/
│   │   │   ├── EventTable.tsx
│   │   │   ├── EventDetail.tsx
│   │   │   ├── FilterBar.tsx
│   │   │   └── PacketInspector.tsx
│   │   ├── hooks/
│   │   │   ├── useEventStream.ts
│   │   │   └── useEventHistory.ts
│   │   └── lib/
│   │       └── event-types.ts
│   ├── index.html
│   ├── vite.config.ts
│   └── package.json
└── dist/
    └── explorer-ui/               # Built frontend (served by Express)
```

## Success Criteria

- [ ] Explorer UI accessible on port 3001 (configurable) for each connector node
- [ ] Real-time event stream displays in <100ms latency
- [ ] All 20+ telemetry event types rendered with type-specific formatting
- [ ] ILP packet OER fields decoded and explained in detail view
- [ ] Event history queryable with filters (1M+ events performant)
- [ ] Search returns results in <500ms for typical queries
- [ ] UI follows shadcn/ui design patterns consistently
- [ ] Docker deployment includes explorer with no extra configuration
- [ ] Mobile-responsive layout for tablet/phone debugging

## Stories

### Story 14.1: Explorer Event Store (libSQL)

Implement libSQL-based event persistence integrated with TelemetryEmitter.

**Acceptance Criteria:**

- EventStore class wraps libSQL with telemetry schema
- TelemetryEmitter writes to EventStore (in addition to WebSocket)
- Events stored with all indexed fields extracted
- Configurable retention policy (max age, max count)
- Query API for filtered event retrieval with pagination
- Unit tests cover all CRUD operations

**Technical Notes:**

- libSQL provides SQLite-compatible storage
- Store full event JSON in `payload` column for flexibility
- Extract key fields to dedicated columns for indexing

---

### Story 14.2: Explorer Server Infrastructure

Create Express server embedded in connector serving UI and WebSocket events.

**Acceptance Criteria:**

- ExplorerServer class initializes on configurable port (default 3001)
- Serves static files from `dist/explorer-ui/`
- WebSocket endpoint `/ws` for real-time event streaming
- REST endpoint `GET /api/events` for historical queries
- Health endpoint `GET /api/health` returns node status
- CORS configured for local development
- Graceful shutdown on connector stop

**Technical Notes:**

- Separate from BTP WebSocket server (different port)
- EventBroadcaster subscribes to TelemetryEmitter
- Rate limiting for REST API (prevent query abuse)

---

### Story 14.3: Explorer UI Foundation

Create React frontend with shadcn/ui components and Vite build tooling.

**Acceptance Criteria:**

- Vite project initialized in `explorer-ui/` directory
- shadcn/ui configured with M2M theme (dark mode default)
- WebSocket hook connects to `/ws` endpoint
- EventTable component displays streaming events
- Basic layout with header showing node ID
- Build produces static bundle in `dist/explorer-ui/`
- npm script integrates UI build into connector build

**Technical Notes:**

- Use `useEventStream` hook for WebSocket connection
- Table uses shadcn/ui DataTable pattern
- Auto-reconnect on WebSocket disconnect

---

### Story 14.4: Event Table and Filtering

Implement full-featured event table with filtering and search.

**Acceptance Criteria:**

- EventTable displays all event types with color-coded badges
- Columns: Time, Type, Direction, Peer, Destination, Amount, Status
- FilterBar component with:
  - Event type multi-select dropdown
  - Time range selector (1m, 5m, 1h, 24h, custom)
  - Direction filter (Sent/Received/All)
  - Text search input
- Filters apply to both live stream and historical queries
- Pagination for historical events (50 per page)
- "Jump to live" button when scrolled in history

**Technical Notes:**

- Use shadcn/ui Select, DatePicker, Input components
- Debounce search input (300ms)
- Virtual scrolling for performance with large event counts

---

### Story 14.5: Event Detail Panel

Create expandable detail view with packet inspection.

**Acceptance Criteria:**

- Clicking event row opens detail panel (slide-out or expansion)
- Raw JSON tab with syntax highlighting
- ILP Packet tab for packet events:
  - Decoded OER fields with labels
  - Amount in human-readable format
  - Condition/Fulfillment hex display
  - Expiry countdown if applicable
- Related Events section links to corresponding Fulfill/Reject
- Copy buttons for packet ID, destination, raw JSON

**Technical Notes:**

- Use shadcn/ui Sheet or Accordion for detail panel
- Reuse OER decoder from `packages/shared`
- Link related events by executionCondition matching

---

### Story 14.6: Settlement and Balance Visualization

Add specialized views for settlement tracking and account balances.

**Acceptance Criteria:**

- Dedicated "Accounts" tab showing all peer account balances
- Account card displays:
  - Current balance (debit/credit/net)
  - Settlement threshold progress bar
  - Recent balance change history
  - Channel status if payment channel active
- Settlement event timeline:
  - TRIGGERED → INITIATED → COMPLETED flow
  - Success/failure indicators
  - Settlement amount and method (MOCK/EVM/XRP)
- Filter event table to show only settlement-related events

**Technical Notes:**

- Account balances from ACCOUNT_BALANCE events
- Aggregate balance changes for trend display
- Link to payment channel events for full context

---

### Story 14.7: Docker Integration and Configuration

Integrate explorer into Docker deployment with configuration options.

**Acceptance Criteria:**

- Dockerfile builds explorer UI as part of connector image
- Explorer port exposed in docker-compose configurations
- Environment variables for configuration:
  - `EXPLORER_ENABLED` (default: true)
  - `EXPLORER_PORT` (default: 3001)
  - `EXPLORER_RETENTION_DAYS` (default: 7)
  - `EXPLORER_MAX_EVENTS` (default: 1000000)
- Health check includes explorer server status
- All topology configs (mesh, hub-spoke, etc.) expose explorer ports
- Documentation for accessing explorer per node

**Technical Notes:**

- Multi-stage Docker build for UI optimization
- Port mapping: alice:3001, bob:3002, charlie:3003, etc.
- Explorer uses dedicated libSQL database for event storage

---

## Compatibility Requirements

- [ ] Existing connector APIs remain unchanged
- [ ] TelemetryEmitter interface unchanged (only adds EventStore consumer)
- [ ] BTP WebSocket server unaffected (separate port)
- [ ] No performance impact on packet processing (<1% overhead)
- [ ] UI build failure does not prevent connector startup (graceful fallback)
- [ ] Works with all existing Docker Compose topologies

## Risk Mitigation

- **Primary Risk**: Event storage consuming excessive disk space
- **Mitigation**: Configurable retention limits; automatic pruning job
- **Rollback Plan**: `EXPLORER_ENABLED=false` disables feature entirely

- **Secondary Risk**: WebSocket connections overwhelming connector
- **Mitigation**: Max connections limit; connection rate limiting
- **Rollback Plan**: Restart connector with explorer disabled

## Definition of Done

- [ ] All 7 stories completed with acceptance criteria met
- [ ] Explorer accessible on each node in all Docker topologies
- [ ] 90%+ code coverage for explorer-specific code
- [ ] Performance benchmarks show <1% overhead on packet processing
- [ ] UI tested across Chrome, Firefox, Safari
- [ ] Mobile-responsive design verified on tablet/phone viewports
- [ ] Documentation updated with explorer usage guide
- [ ] No regression in existing connector functionality

## Dependencies

- **Epic 6**: Settlement Foundation & Accounting (settlement event types)
- **Epic 8/9/27**: Payment Channels (channel event types)

## Technical References

- **Telemetry Types**: `packages/shared/src/types/telemetry.ts`
- **TelemetryEmitter**: `packages/connector/src/telemetry/telemetry-emitter.ts`
- **libSQL**: SQLite-compatible event storage
- **shadcn/ui**: https://ui.shadcn.com/docs (v4)

---

**Epic Status:** Completed

**Estimated Stories:** 7

**Architecture Reference:** Extends existing telemetry infrastructure
