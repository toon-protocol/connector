# Epic 18: Explorer UI — Network Operations Center Redesign

**Epic Number:** 18

**Goal:** Transform the Connector Explorer into a distinctive, production-grade Network Operations Center (NOC) dashboard using the frontend-design skill and Playwright MCP verification. Deliver a modern, visually striking interface with a Dashboard-first approach that emphasizes real-time ILP packet routing metrics, live packet flow visualization, and comprehensive observability across all five tabs (Dashboard, Packets, Accounts, Peers, Keys) with seamless live and historical data integration.

**Foundation:** This epic builds on Epic 11 (Packet/Event Explorer UI) and Epic 12 (Explorer Polish), which established the WebSocket streaming infrastructure, event persistence with libSQL, shadcn/ui v4 component library, and the core tab structure. The redesign leverages the existing telemetry pipeline (TelemetryEmitter → EventStore → EventBroadcaster → WebSocket) while introducing a bold new aesthetic direction and improved information architecture.

**Design Philosophy:** The "Network Operations Center" aesthetic draws inspiration from professional monitoring systems (trading terminals, air traffic control, network NOCs) to create a serious, technical interface appropriate for infrastructure monitoring. Key characteristics:

- **Deep space background** (#0D1829) optimized for 24/7 viewing
- **Neon accent colors** for ILP packet types (Cyan=PREPARE, Emerald=FULFILL, Rose=REJECT)
- **Monospace typography** for all technical data (addresses, IDs, amounts)
- **Real-time emphasis** with pulse animations, live indicators, and streaming feeds
- **Metrics-first approach** with prominent KPIs on the Dashboard landing page

**Reference:**

- Frontend-design skill: `~/.claude/plugins/cache/claude-plugins-official/frontend-design/`
- Playwright MCP server for UI verification
- Existing Explorer UI: `packages/connector/explorer-ui/`
- Deployment script: `scripts/deploy-5-peer-multihop.sh`
- Epic 11 foundation: `docs/prd/epic-11-packet-event-explorer-ui.md`
- Epic 12 polish: `docs/prd/epic-12-agent-explorer-polish.md`

---

## Story 18.1: Dashboard Landing Page with NOC Aesthetic

As a connector operator,
I want a Dashboard tab as the default landing page showing key metrics and live packet flow,
so that I can assess my node's health and routing performance at a glance.

**Prerequisites:** Epic 11 telemetry infrastructure, Epic 12 event streaming

### Acceptance Criteria

1. **Dashboard Tab** added to navigation, becomes default landing page (keyboard shortcut: `1`)
2. **Hero Metrics Grid** displays four key performance indicators:
   - Total Packets (lifetime count with activity icon)
   - Success Rate (FULFILL/REJECT ratio, color-coded: green >90%, yellow >70%, red <70%)
   - Active Channels (number of open payment channels)
   - Routing Status (connection state with pulse animation when active)
3. **Packet Distribution Section** shows visual breakdown:
   - PREPARE count with cyan progress bar
   - FULFILL count with emerald progress bar
   - REJECT count with rose progress bar
   - Percentage distribution calculated from event stream
4. **Live Packet Flow** displays recent packets (10 most recent):
   - Each packet shows: type badge, from/to addresses, destination, amount, timestamp
   - Animated slide-in effect for new packets
   - Color-coded by packet type (cyan/emerald/rose)
   - Empty state: "Waiting for packet activity..." with pulse icon
5. **Real-time Updates** via WebSocket:
   - Metrics recalculate on each new event
   - Packet flow prepends new packets to list
   - Success rate updates automatically
6. **NOC Color Palette** applied:
   - Background: `hsl(222, 47%, 6%)` (deep space)
   - Cards: `hsl(222, 47%, 8%)` (elevated)
   - Borders: `hsl(217, 33%, 14%)` (subtle)
   - Packet type colors: Cyan (#06B6D4), Emerald (#10B981), Rose (#F43F5E)
7. **Responsive Layout** works on desktop (1920x1080+) and tablet (1024x768+)
8. **Dashboard Component** created: `packages/connector/explorer-ui/src/components/Dashboard.tsx`
9. **Playwright Verification**:
   - Deploy 5-peer network using `deploy-5-peer-multihop.sh`
   - Send test packets to generate activity
   - Screenshot captured showing populated Dashboard
   - Verify metrics update in real-time
10. **Unit Tests** for metric calculations, packet flow formatting, success rate computation

### Technical Design

```typescript
// packages/connector/explorer-ui/src/components/Dashboard.tsx

interface DashboardProps {
  events: TelemetryEvent[];
  connectionStatus: 'connecting' | 'connected' | 'disconnected' | 'error';
}

interface DashboardMetrics {
  totalPackets: number;
  prepareCount: number;
  fulfillCount: number;
  rejectCount: number;
  successRate: number;
  activeChannels: number; // TODO: Fetch from API
}

export function Dashboard({ events, connectionStatus }: DashboardProps) {
  const metrics = useMemo(() => calculateMetrics(events), [events]);
  const packetFlow = useMemo(() => extractPacketFlow(events, 10), [events]);

  return (
    <div className="space-y-6">
      <MetricsGrid metrics={metrics} connectionStatus={connectionStatus} />
      <PacketDistribution metrics={metrics} />
      <LivePacketFlow packets={packetFlow} />
    </div>
  );
}
```

### UI Layout

```
┌─────────────────────────────────────────────────────────────────────┐
│ ⚡ ILP CONNECTOR          peer1    0h 42m    ● CONNECTED  8:45:23 AM│
├─────────────────────────────────────────────────────────────────────┤
│ View: [●Live | ⏱History]  ● Streaming                               │
├─────────────────────────────────────────────────────────────────────┤
│ [Dashboard] [Packets] [Accounts] [Peers] [Keys]                     │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ┌──────────────┐ ┌──────────────┐ ┌──────────────┐ ┌────────────┐│
│  │TOTAL PACKETS │ │ SUCCESS RATE │ │ACTIVE CHANNELS│ │   ROUTING  ││
│  │     12,543   │ │    94.2%     │ │      5       │ │   Active   ││
│  │All-time route│ │11,819 / 724  │ │Payment chs   │ │Accepting   ││
│  └──────────────┘ └──────────────┘ └──────────────┘ └────────────┘│
│                                                                      │
│  Packet Distribution                                                │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │ ● PREPARE  ████████████████░░░░  80% (10,034)              │   │
│  │ ● FULFILL  ███████████████░░░░░  75% (9,420)               │   │
│  │ ● REJECT   ███░░░░░░░░░░░░░░░░░  15% (1,889)               │   │
│  └─────────────────────────────────────────────────────────────┘   │
│                                                                      │
│  Live Packet Flow                                            [Live] │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │ ● [PREPARE] → g.peer1 ⟶ g.peer2  │ 1.5M  │ 2s ago         │   │
│  │ ● [FULFILL] → g.peer2 ⟵ g.peer3  │ 1.5M  │ 2s ago         │   │
│  │ ● [PREPARE] → g.peer1 ⟶ g.peer3  │ 2.3M  │ 5s ago         │   │
│  └─────────────────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────────────────┘
```

---

## Story 18.2: Enhanced Header with Technical Branding

As a connector operator,
I want a professionally branded header with real-time status indicators,
so that I immediately understand my node's identity and operational state.

**Prerequisites:** Story 18.1

### Acceptance Criteria

1. **ILP Connector Branding** prominent in header:
   - Lightning bolt icon (⚡) with live status indicator dot
   - "ILP CONNECTOR" title in uppercase, monospace styling
   - "Network Operations" subtitle
2. **Node Identity** displayed:
   - Node ID from health API (`/api/health`)
   - Uptime in hours/minutes format (e.g., "3h 42m")
3. **Connection Status** with visual indicator:
   - Emerald dot + "CONNECTED" when WebSocket active
   - Yellow dot + "CONNECTING" during connection attempt
   - Gray dot + "DISCONNECTED" when offline
   - Red dot + "ERROR" on connection failure
   - Pulse animation on active connection
4. **Real-Time Clock** displaying system time (HH:MM:SS AM/PM format, updates every second)
5. **Event Count** badge showing total events in current session
6. **Keyboard Shortcuts Button** (`?` key) to display help dialog
7. **Gradient Background** with subtle scan-line effect for NOC aesthetic
8. **Monospace Typography** for all technical data (Node ID, uptime, timestamps)
9. **Responsive Layout** adapts to screen width (hides uptime/clock on mobile)
10. **Playwright Verification**:
    - Header renders with all elements
    - Clock updates every second
    - Status indicator changes color based on connection state
    - Screenshot captured for visual regression

### Technical Design

```typescript
// packages/connector/explorer-ui/src/components/Header.tsx

interface HeaderProps {
  status: 'connecting' | 'connected' | 'disconnected' | 'error';
  eventCount: number;
  onHelpOpen?: () => void;
}

export const Header = memo(function Header({ status, eventCount, onHelpOpen }: HeaderProps) {
  const [health, setHealth] = useState<HealthResponse | null>(null);
  const [currentTime, setCurrentTime] = useState(new Date());

  // Fetch health every 30s
  // Update clock every 1s

  return (
    <header className="border-b bg-gradient-to-r from-background via-card/30 to-background">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-6">
          <div className="relative">
            <Zap className="h-7 w-7 text-cyan-500" />
            {status === 'connected' && <Circle className="pulse-dot" />}
          </div>
          <div>
            <h1 className="font-mono text-xl font-bold tracking-tight">ILP CONNECTOR</h1>
            <p className="text-xs text-muted-foreground uppercase">Network Operations</p>
          </div>
          <div className="flex gap-3 border-l pl-6">
            <div>
              <p className="text-xs text-muted-foreground uppercase">Node ID</p>
              <p className="font-mono text-sm">{health?.nodeId}</p>
            </div>
            <div>
              <p className="text-xs text-muted-foreground uppercase">Uptime</p>
              <p className="font-mono text-sm">{formatUptime(health?.uptime)}</p>
            </div>
          </div>
        </div>
        <div className="flex items-center gap-4">
          <Badge variant="outline" className="font-mono">Events {eventCount}</Badge>
          <StatusIndicator status={status} />
          <SystemClock time={currentTime} />
          <Button variant="ghost" size="icon" onClick={onHelpOpen}>
            <Keyboard className="h-4 w-4" />
          </Button>
        </div>
      </div>
    </header>
  );
});
```

---

## Story 18.3: Packets Tab Redesign with ILP Terminology

As a connector operator,
I want the Packets tab to emphasize ILP packet types and routing flow,
so that I can quickly understand packet forwarding behavior.

**Prerequisites:** Story 18.1, Story 18.2

### Acceptance Criteria

1. **Tab Renamed** from "Events" to "Packets" (keyboard shortcut: `2`)
2. **Packet Type Prominence** in table:
   - PREPARE/FULFILL/REJECT displayed as primary badge (cyan/emerald/rose)
   - Event action (received/forwarded/sent) as secondary label below
   - Packet type colors match Dashboard color scheme
3. **Routing Flow Visualization**:
   - From → To addresses with directional arrows
   - Destination ILP address prominently displayed
   - Clickable peer links open peer's Explorer in new tab
4. **Status Column** enhanced:
   - Success (✓) for FULFILL packets
   - Failure (✗) for REJECT packets
   - Pending (◐) for unresolved PREPARE packets
   - Neutral (○) for non-packet events
5. **Filter Bar** updated:
   - "ILP Packets" filter category groups PACKET_RECEIVED, PACKET_FORWARDED, AGENT_CHANNEL_PAYMENT_SENT
   - Quick filter presets for packet types
   - Settlement filter retained from Epic 12
6. **Empty States** improved:
   - "Waiting for packet activity..." with pulse icon when no packets
   - "No packets match your filters" when filters active
7. **Virtual Scrolling** performance maintained (60fps with 1000+ events)
8. **Packet Detail Panel** updated:
   - Packet type badge prominent at top
   - From/To/Destination in monospace font
   - Amount formatted with abbreviations (K/M/B/T)
   - Related packets linked (PREPARE → FULFILL/REJECT correlation)
9. **Playwright Verification**:
   - Send packets through 5-peer network
   - Verify packet table populates
   - Test filter interactions
   - Screenshot captured with live packet flow
10. **Unit Tests** for packet type detection, status computation, filter logic

### UI Layout

```
┌─────────────────────────────────────────────────────────────────────┐
│ [Dashboard] [●Packets] [Accounts] [Peers] [Keys]                    │
├─────────────────────────────────────────────────────────────────────┤
│ Filters:                                                             │
│ Type: [All event types ▼] [⚡ ILP Packets] [💰 Settlement]          │
│ Direction: [All ▼]  Search: [🔍 ________________]                   │
│ Time: [1m] [5m] [1h] [24h] [📅 Custom]  All time                    │
├──────┬────────┬─────────┬────────┬───────────────┬────────┬────────┤
│ Time │ Type   │ From    │ To     │ Destination   │ Amount │ Status │
├──────┼────────┼─────────┼────────┼───────────────┼────────┼────────┤
│ 2s   │PREPARE │g.peer1  │g.peer2 │g.peer5.dest   │ 1.5M   │ ✓ OK   │
│      │received│         │        │               │        │        │
├──────┼────────┼─────────┼────────┼───────────────┼────────┼────────┤
│ 2s   │FULFILL │g.peer2  │g.peer1 │g.peer5.dest   │ 1.5M   │ ✓ OK   │
│      │received│         │        │               │        │        │
└──────┴────────┴─────────┴────────┴───────────────┴────────┴────────┘
```

---

## Story 18.4: Accounts Tab Visualization Enhancement

As a connector operator,
I want the Accounts tab to clearly show peer balances and settlement state,
so that I can monitor credit exposure and settlement health.

**Prerequisites:** Story 18.1, Epic 12 accounts infrastructure

### Acceptance Criteria

1. **Account Cards** redesigned with NOC aesthetic:
   - Peer ID as card title with monospace font
   - Net balance prominently displayed (large, bold, tabular-nums)
   - Debit/Credit breakdown with color coding (red debit, green credit)
   - Settlement state badge (IDLE/PENDING/IN_PROGRESS) with color
   - Settlement threshold progress bar
2. **Balance History Chart** enhanced:
   - Area chart with gradient fill (emerald for positive, rose for negative)
   - Time axis with relative timestamps
   - Hover tooltip showing exact balance and timestamp
   - Zero baseline clearly marked
3. **Channel State Integration**:
   - Active payment channel indicator (if hasActiveChannel=true)
   - Channel type badge (EVM/XRP/Aptos) with blockchain icon
   - Channel balance vs account balance comparison
4. **Settlement Timeline** visual:
   - Chronological list of settlement events
   - SETTLEMENT_TRIGGERED → SETTLEMENT_COMPLETED flow
   - Time elapsed between trigger and completion
   - Settlement amounts with blockchain confirmation links
5. **Empty State**: "No peer accounts yet" with setup instructions
6. **Real-Time Updates**: Balances update on new ACCOUNT_BALANCE events
7. **Historical Hydration**: Accounts populate from `/api/accounts/events` on mount
8. **Responsive Grid**: 1 column (mobile), 2 columns (tablet), 3 columns (desktop)
9. **Playwright Verification**:
   - Run integration test generating settlement activity
   - Verify accounts populate with balances
   - Test balance history chart renders
   - Screenshot captured with active accounts
10. **Unit Tests** for balance calculations, settlement state logic, chart data formatting

---

## Story 18.5: Peers Tab Network Topology View

As a connector operator,
I want the Peers tab to show my node's routing topology and peer connections,
so that I understand my position in the ILP network.

**Prerequisites:** Story 18.1, Epic 12 peers API

### Acceptance Criteria

1. **Peers Table** displays connected peers:
   - Peer ID with status indicator (green=connected, gray=disconnected)
   - ILP Address in monospace font
   - EVM Address (if configured) with blockchain explorer link
   - XRP Address (if configured) with blockchain explorer link
   - Aptos Address (if configured) with blockchain explorer link
   - BTP URL showing WebSocket endpoint
   - Connection state (Connected/Disconnected) with uptime
2. **Routing Table** shows route entries:
   - ILP Address prefix (destination pattern)
   - Next Hop peer ID
   - Priority/preference value
   - Route type (static/dynamic)
3. **Network Topology Visualization** (optional, stretch goal):
   - Node graph showing this node + connected peers
   - Directional edges for packet routes
   - Animated pulse on active routes
4. **Peer Health Indicators**:
   - Last packet timestamp ("Active 2s ago")
   - Packet success rate to this peer
   - Settlement status (if applicable)
5. **Empty State**: "No peers connected" with peering setup instructions
6. **Real-Time Updates**: Peer list refreshes on connection changes
7. **Data Fetching**:
   - GET `/api/peers` for peer connection data
   - GET `/api/routes` for routing table entries
8. **Responsive Layout**: Table scrolls horizontally on mobile
9. **Playwright Verification**:
   - Deploy 5-peer network
   - Verify peers table shows peer1→peer2 and peer1→sendPacketClient
   - Test blockchain address links
   - Screenshot captured with populated peer list
10. **Unit Tests** for peer data parsing, route table formatting, link generation

---

## Story 18.6: Keys Tab Security Management Interface

As a connector operator,
I want the Keys tab to manage my node's cryptographic keys and security settings,
so that I can configure signing keys for payment channels.

**Prerequisites:** Story 18.1, Epic 12 KeyManager component

### Acceptance Criteria

1. **Key Management Interface** (retain from Epic 12):
   - EVM keypair display (address, public key)
   - XRP keypair display (address, public key)
   - Aptos keypair display (address, public key)
   - Key generation buttons for each blockchain
   - Export/import functionality
2. **NOC Styling Applied**:
   - Keys displayed in monospace font
   - Address cards with subtle borders
   - Copy-to-clipboard buttons with visual feedback
   - Warning/info messages with appropriate colors
3. **Security Status Indicators**:
   - Key present (green checkmark)
   - Key missing (yellow warning)
   - Key configuration error (red alert)
4. **Key Metadata**:
   - Key creation timestamp
   - Last used timestamp (if available)
   - Associated channels count
5. **Empty State**: "No keys configured" with generation instructions
6. **Responsive Layout**: Stacked on mobile, side-by-side on desktop
7. **Data Persistence**: Keys stored in secure location (existing KeyManager logic)
8. **Playwright Verification**:
   - Open Keys tab
   - Verify key display formatting
   - Test copy-to-clipboard functionality
   - Screenshot captured
9. **Unit Tests** for key display formatting, validation logic
10. **Security Audit**: No private keys logged or exposed in UI

---

## Story 18.7: Custom CSS Animations and Effects

As a connector operator,
I want subtle animations and visual effects throughout the UI,
so that the interface feels alive and responsive.

**Prerequisites:** Stories 18.1-18.6

### Acceptance Criteria

1. **Pulse Animations**:
   - Live indicators pulse gently (connection status, "Live" badges)
   - Pulse glow keyframes defined in CSS
   - Duration: 2s, ease-in-out timing
2. **Slide-In Animations**:
   - New packets slide in from left with fade effect
   - Dashboard cards fade in on mount
   - Duration: 400ms, cubic-bezier easing
3. **Hover Effects**:
   - Cards elevate with subtle shadow on hover
   - Buttons show color transition (200ms)
   - Links underline on hover
4. **Loading States**:
   - Skeleton loaders pulse during data fetch
   - Spinner for async operations
   - Progress bars animate smoothly
5. **Status Transitions**:
   - Success rate card color fades between green/yellow/red states
   - Connection status dot animates color changes
6. **Custom CSS Classes** in `index.css`:
   - `.pulse-glow` - Pulsing glow effect
   - `.slide-in-from-left` - Entry animation
   - `.fade-in` - Opacity fade
   - `.grid-pattern` - Subtle background grid
   - `.glow-cyan`, `.glow-emerald`, `.glow-rose` - Neon glow effects
7. **Animation Preferences**: Respect `prefers-reduced-motion` media query
8. **Performance**: No jank, 60fps maintained during animations
9. **Playwright Verification**:
   - Record video of animations in action
   - Verify smooth transitions
   - Test with reduced-motion preference
10. **Cross-Browser Testing**: Animations work in Chrome, Firefox, Safari

### CSS Implementation

```css
/* packages/connector/explorer-ui/src/index.css */

@keyframes pulse-glow {
  0%,
  100% {
    opacity: 1;
    filter: brightness(1);
  }
  50% {
    opacity: 0.8;
    filter: brightness(1.2);
  }
}

@keyframes slide-in-from-left {
  from {
    opacity: 0;
    transform: translateX(-12px);
  }
  to {
    opacity: 1;
    transform: translateX(0);
  }
}

.pulse-glow {
  animation: pulse-glow 2s ease-in-out infinite;
}

.slide-in-from-left {
  animation: slide-in-from-left 400ms cubic-bezier(0.16, 1, 0.3, 1);
}

.grid-pattern {
  background-image:
    linear-gradient(rgba(255, 255, 255, 0.02) 1px, transparent 1px),
    linear-gradient(90deg, rgba(255, 255, 255, 0.02) 1px, transparent 1px);
  background-size: 20px 20px;
}

.glow-cyan {
  text-shadow: 0 0 8px rgba(6, 182, 212, 0.5);
}

@media (prefers-reduced-motion: reduce) {
  *,
  *::before,
  *::after {
    animation-duration: 0.01ms !important;
    animation-iteration-count: 1 !important;
    transition-duration: 0.01ms !important;
  }
}
```

---

## Story 18.8: Playwright MCP Integration Testing

As a developer,
I want automated visual regression tests using Playwright MCP,
so that UI changes are verified against real multi-node network data.

**Prerequisites:** Stories 18.1-18.7, Playwright MCP server configured

### Acceptance Criteria

1. **Test Deployment Script** verified:
   - `scripts/deploy-5-peer-multihop.sh` runs successfully
   - All 5 peers start and report healthy
   - Peers funded with test tokens
   - WebSocket connections established between peers
2. **Packet Generation** automated:
   - Send 10+ packets through the network (peer1 → peer5)
   - Varying amounts to show distribution
   - Mix of successful and failed packets (if possible)
3. **Playwright Test Suite** created:
   - Navigate to each peer's Explorer (localhost:5173-5177)
   - Screenshot Dashboard tab with populated metrics
   - Screenshot Packets tab with event table
   - Screenshot Accounts tab with peer accounts
   - Screenshot Peers tab with peer list
   - Screenshot Keys tab with key display
4. **Visual Regression Baseline**:
   - Screenshots saved as baseline images
   - Future runs compare against baseline
   - Diff highlighted if UI changes
5. **Interaction Testing**:
   - Test tab navigation (click and keyboard shortcuts)
   - Test filter interactions on Packets tab
   - Test search functionality
   - Test packet detail panel open/close
6. **Connection State Testing**:
   - Verify "Connected" status when active
   - Test reconnection behavior (stop/start peer)
   - Screenshot "Disconnected" state
7. **Data Validation**:
   - Assert metrics values are non-zero after packet sends
   - Assert packet table contains expected packets
   - Assert peer list shows peer2 from peer1's perspective
8. **Performance Testing**:
   - Measure page load time
   - Measure time to first render
   - Measure WebSocket connection time
   - Assert all metrics < acceptable thresholds
9. **Test Documentation**:
   - README with setup instructions
   - Example commands for running tests
   - Troubleshooting guide
10. **CI Integration** (stretch goal):
    - Tests run on pull requests
    - Screenshots uploaded as artifacts
    - Test failures block merge

### Test Structure

```typescript
// packages/connector/explorer-ui/playwright/explorer.spec.ts

import { test, expect } from '@playwright/test';

test.describe('Explorer UI - Dashboard', () => {
  test.beforeAll(async () => {
    // Deploy 5-peer network
    // Send test packets
  });

  test('Dashboard shows metrics', async ({ page }) => {
    await page.goto('http://localhost:5173');
    await expect(page.getByRole('tab', { name: 'Dashboard' })).toBeVisible();

    // Assert metrics visible
    await expect(page.getByText('TOTAL PACKETS')).toBeVisible();
    await expect(page.getByText('SUCCESS RATE')).toBeVisible();

    // Screenshot for visual regression
    await page.screenshot({ path: 'screenshots/dashboard.png' });
  });

  test('Live packet flow updates', async ({ page }) => {
    await page.goto('http://localhost:5173');

    // Wait for packet to appear
    await page.waitForSelector('[data-testid="packet-flow-item"]');

    // Assert packet details visible
    await expect(page.getByText('PREPARE')).toBeVisible();
  });
});
```

---

## Story 18.9: Documentation and Migration Guide

As a developer or operator,
I want documentation explaining the new UI design and features,
so that I can effectively use and maintain the Explorer.

**Prerequisites:** Stories 18.1-18.8 completed

### Acceptance Criteria

1. **Redesign Documentation** created: `docs/explorer/redesign-guide.md`
   - Design philosophy and aesthetic rationale
   - Color palette reference
   - Typography scale
   - Component hierarchy
   - Animation catalog
2. **User Guide** updated: `docs/explorer/user-guide.md`
   - Dashboard overview with screenshots
   - Keyboard shortcuts reference
   - Filter usage examples
   - Tab descriptions (Dashboard, Packets, Accounts, Peers, Keys)
   - Common workflows (monitoring, debugging, troubleshooting)
3. **Developer Guide** updated: `docs/explorer/developer-guide.md`
   - Component architecture
   - Data flow diagrams (TelemetryEmitter → WebSocket → React)
   - Adding new metrics to Dashboard
   - Customizing color themes
   - Performance optimization tips
4. **Migration Notes** for existing users:
   - Dashboard now default landing page
   - Events tab renamed to Packets
   - New keyboard shortcuts
   - Breaking changes (if any)
5. **Deployment Guide** updated:
   - Explorer port configuration
   - Environment variables
   - Docker Compose examples
   - Multi-node deployment tips
6. **Troubleshooting Section**:
   - WebSocket connection issues
   - Metrics not updating
   - Empty state debugging
   - Performance problems
7. **Screenshots** in documentation:
   - All five tabs captured
   - Before/after comparison (Epic 12 vs Epic 18)
   - Different screen sizes (desktop/tablet)
8. **Changelog Entry** in `CHANGELOG.md`:
   - Major: Dashboard redesign with NOC aesthetic
   - Added: Live packet flow visualization
   - Changed: Events tab renamed to Packets
   - Improved: All tabs with modern styling
9. **README Update** in `packages/connector/explorer-ui/README.md`:
   - Quick start guide
   - Development instructions
   - Building for production
   - Testing with Playwright
10. **Video Demo** (optional, stretch goal):
    - Screen recording showing all features
    - Walkthrough of Dashboard, Packets, Accounts, Peers, Keys
    - Live packet routing demonstration
    - Posted to project documentation site

---

## Epic Summary

This epic transforms the Connector Explorer from a functional monitoring tool into a distinctive, production-grade Network Operations Center interface. The redesign emphasizes real-time ILP packet routing with a Dashboard-first approach, modern NOC aesthetic, and comprehensive Playwright MCP verification.

**Key Deliverables:**

1. ✅ Dashboard landing page with metrics, packet distribution, and live packet flow
2. ✅ Enhanced header with technical branding and real-time indicators
3. ✅ Packets tab redesigned with ILP packet type prominence
4. ✅ Accounts tab visualization improvements
5. ✅ Peers tab network topology view
6. ✅ Keys tab security management interface
7. ✅ Custom CSS animations and NOC effects
8. ✅ Playwright MCP integration testing suite
9. ✅ Comprehensive documentation and migration guide

**Success Metrics:**

- Dashboard loads < 1s on first paint
- All tabs verified with real 5-peer network data
- 100% visual coverage with Playwright screenshots
- User feedback indicates improved usability and aesthetics
- Zero regressions in existing functionality

**Dependencies:**

- Epic 11: Packet/Event Explorer UI (foundation)
- Epic 12: Explorer Polish (WebSocket, shadcn/ui, accounts)
- Frontend-design skill: UI component generation
- Playwright MCP server: Automated UI verification
- `deploy-5-peer-multihop.sh`: Test network deployment

**Timeline Estimate:** 3-4 weeks (9 stories, medium-high complexity)

**Risk Mitigation:**

- Visual regression risk: Playwright screenshots catch unintended changes
- Performance risk: Virtual scrolling and memoization prevent slowdowns
- Data integration risk: Reuse existing telemetry pipeline (no backend changes)
- Browser compatibility risk: Test in Chrome, Firefox, Safari
- Accessibility risk: Maintain WCAG AA contrast, keyboard navigation
