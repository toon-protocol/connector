# Epic 12: Connector Explorer — Performance, UX & Visual Quality

## Brownfield Enhancement

This epic enhances the existing Packet/Event Explorer UI (delivered in Epic 11) to improve performance, user experience, and visual quality. It also rebrands from "Explorer" to "Connector Explorer" and establishes the Docker integration test as the canonical data source for development and verification.

## Epic Goal

Transform the functional Explorer into a polished, high-performance observability tool branded as **Connector Explorer**, verified against real multi-node network data from the Docker integration test, with measurable improvements to render performance, interaction design, and visual consistency.

## Epic Description

### Existing System Context

- **Current functionality:** Per-node web-based Explorer embedded in each connector (Epic 11), featuring real-time WebSocket event streaming, historical REST API queries, event table with filtering, detail panel with packet/packet inspection, accounts view with settlement tracking, and on-chain wallet visibility.
- **Technology stack:** React 18, Vite 6, TypeScript, shadcn/ui v4, TailwindCSS, @tanstack/react-virtual, WebSocket (ws), Express, libSQL.
- **Integration points:** TelemetryEmitter → EventStore → EventBroadcaster → WebSocket → React hooks (useEventStream, useEvents, useAccountBalances, useWalletBalances).

### Enhancement Details

**What's being changed:**

1. **Rebranding:** All references to "Explorer" changed to "Connector Explorer" — HTML title, header component, package name display, and any user-facing text.
2. **Real Data Verification:** All UI work developed and verified using live data from the Docker integration integration test (`docker-agent-society.test.ts`), ensuring every view renders correctly with authentic multi-node telemetry (ILP packets, packet events, settlement flows, payment channels).
3. **Performance Optimization:** Reduce render overhead for high-volume event streams, optimize virtual scrolling, minimize unnecessary re-renders, and improve WebSocket message batching.
4. **User Experience:** Improve navigation flow, add keyboard shortcuts for power users, improve responsive behavior, refine filter interactions, and add contextual empty states.
5. **Visual Quality:** Audit and refine typography scale, spacing consistency, color palette usage, animation timing, component alignment, and dark theme contrast ratios.

6. **Historical Data Hydration:** Accounts and payment channels views hydrate from historical event data on page load, so the Explorer works correctly when opened after events have already occurred (not just live WebSocket).
7. **Peers & Routing View:** New Peers tab showing connected peers with ILP addresses, on-chain addresses, BTP connection status, and routing table entries for network topology visibility.

**How it integrates:** All changes are within the existing `packages/connector/explorer-ui/` frontend and the `packages/connector/src/explorer/` backend. Stories 15.5 and 15.6 add new REST API endpoints to the explorer server for historical event replay, peer data, and routing table access.

**Success criteria:**

- All "Explorer" references replaced with "Connector Explorer"
- Event table renders 1000+ events at 60fps with no jank
- All views verified against Docker integration test data showing real ILP packets, settlements, and payment channels
- Lighthouse accessibility score ≥ 90
- Visual consistency audit passes (no orphaned styles, consistent spacing, proper contrast)
- Keyboard navigation works for core workflows (event selection, tab switching, filter toggling)
- Accounts tab populates from historical events (peer accounts and payment channels visible after test completes)
- Peers tab shows all connected peers with ILP addresses and routing table

---

## Stories

### Story 15.1: Rebrand to Connector Explorer & Docker Test Data Harness

**Goal:** Rename all user-facing "Explorer" references to "Connector Explorer" and establish a development workflow for running the Explorer against real Docker integration test data.

**Scope:**

- Rename "Explorer" → "Connector Explorer" in: `index.html` title, `Header.tsx` heading, any component text, and `package.json` display name
- Document the workflow for launching the Docker integration test and connecting the Explorer to a running agent's port for visual development/QA
- Create a dev convenience script (e.g., `npm run dev:agent-explorer`) that builds and opens the Explorer pointed at a running docker agent node
- Verify Explorer displays real connector data: ILP packets with packet payloads, settlement events, payment channel opens/updates, wallet balances

**Acceptance Criteria:**

- [ ] No remaining references to "Explorer" in user-facing UI text
- [ ] HTML page title reads "Connector Explorer"
- [ ] Header displays "Connector Explorer" branding
- [ ] Developer documentation explains how to run Docker integration test and connect Explorer
- [ ] Screenshot verification: Explorer rendering real docker agent test data (events, accounts, channels)

**Technical Notes:**

- The Docker integration test (`packages/connector/test/integration/docker-agent-society.test.ts`) spins up N agents with explorers on ports 3100+index
- Vite dev server proxy can target any running agent's explorer port
- Favicon/branding update is optional scope stretch

---

### Story 15.2: Performance Optimization — Render & Streaming

**Goal:** Achieve smooth 60fps rendering with 1000+ events and optimize WebSocket message handling for high-throughput scenarios.

**Scope:**

- Profile React render performance using React DevTools and identify expensive re-renders
- Optimize `EventTable` virtual scrolling: ensure row height calculation is stable, reduce layout thrash, memoize row components
- Implement WebSocket message batching: buffer incoming events over a short window (e.g., 16ms / one frame) and apply as a single state update instead of per-message re-renders
- Memoize filter computations and event transformations with `useMemo` / `useCallback` where profiling shows benefit
- Optimize `AccountsView` re-renders: ensure balance updates don't trigger full account list re-render
- Add React.memo to pure display components (Badge renderers, status indicators)

**Acceptance Criteria:**

- [ ] EventTable maintains 60fps scroll with 1000 events (Chrome DevTools Performance panel)
- [ ] WebSocket burst of 100 events/second does not cause frame drops
- [ ] AccountsView updates individual cards without re-rendering siblings
- [ ] No React strict mode double-render warnings in development
- [ ] Bundle size does not increase by more than 5%

**Technical Notes:**

- Use `requestAnimationFrame` batching pattern for WebSocket events
- @tanstack/react-virtual already provides virtualization — focus on row memoization and stable key generation
- Verify with Docker integration test generating sustained event streams

---

### Story 15.3: User Experience — Navigation, Keyboard & Responsiveness

**Goal:** Improve interaction design so the Explorer feels intuitive for both casual inspection and power-user debugging workflows.

**Scope:**

- **Keyboard shortcuts:**
  - `j`/`k` or `↑`/`↓` to navigate event rows
  - `Enter` to open detail panel for selected event
  - `Escape` to close detail panel
  - `1`/`2` to switch between Events/Accounts tabs
  - `/` to focus search input
- **Filter UX improvements:**
  - Show active filter count badge on filter bar
  - "Clear all filters" button prominently visible when filters are active
  - Persist filter state in URL query parameters so filters survive page refresh
- **Responsive layout:**
  - Audit and fix layout at common breakpoints (768px, 1024px, 1440px)
  - Event detail panel: full-screen overlay on mobile, side sheet on desktop
  - Account cards: responsive grid (1 col mobile, 2 cols tablet, 3+ cols desktop)
- **Empty states:**
  - Show meaningful empty state when no events match filters ("No events match your filters — try adjusting or clearing them")
  - Show welcome state on first load with no events ("Waiting for events... Connector Explorer is connected and listening")
- **Mode switching:**
  - Clearer visual distinction between Live and History modes
  - Auto-switch to Live mode when new events arrive while in History with no scroll position

**Acceptance Criteria:**

- [ ] All keyboard shortcuts functional and discoverable (tooltip or help overlay)
- [ ] Filters persist in URL (bookmarkable/shareable)
- [ ] Layout renders correctly at 768px, 1024px, and 1440px+ widths
- [ ] Detail panel adapts to viewport (overlay on small, sheet on large)
- [ ] Empty states display for: no events, no filter matches, disconnected
- [ ] Verified with Docker integration test data at each breakpoint

**Technical Notes:**

- Use `useSearchParams` from React Router (or URL API directly if no router) for filter persistence
- Keyboard handling via `useEffect` with `keydown` listeners, guarded against input field focus
- Test responsive behavior with Playwright MCP `browser_resize`

---

### Story 15.4: Visual Quality — Design Audit & Polish

**Goal:** Achieve a visually cohesive, professional-quality dark-theme UI that is consistent across all Explorer views.

**Scope:**

- **Typography audit:**
  - Establish consistent type scale (headings, body, mono, captions)
  - Ensure all event type badges, timestamps, amounts use consistent font sizing
  - Fix any inconsistent font-weight or line-height across components
- **Spacing & alignment:**
  - Audit padding/margin consistency across Header, FilterBar, EventTable, AccountsView
  - Ensure card spacing in AccountsView follows consistent gap rhythm
  - Fix any visual misalignment between table columns and header labels
- **Color & contrast:**
  - Verify all text meets WCAG AA contrast ratios against dark background
  - Audit event type badge colors for visual distinctiveness and consistency
  - Ensure status indicators (connected/disconnected/error) use consistent semantic colors
  - Review muted-foreground text legibility
- **Animation & transitions:**
  - Add subtle entry animation for new events appearing in live mode (fade-in or slide)
  - Smooth transitions for detail panel open/close
  - Loading states: skeleton loaders instead of plain "Loading..." text
  - Connection status transitions (connecting → connected) animated
- **Component polish:**
  - Consistent border-radius usage across cards, badges, buttons
  - Hover states on all interactive elements
  - Focus rings visible for keyboard navigation (accessibility)
  - Scroll shadows on EventTable to indicate scrollable content

**Acceptance Criteria:**

- [ ] Typography scale documented and applied consistently across all components
- [ ] No spacing inconsistencies visible at 1440px viewport (pixel audit)
- [ ] All text passes WCAG AA contrast (4.5:1 for normal text, 3:1 for large text)
- [ ] New live events animate into the table (not just appearing)
- [ ] Skeleton loaders shown during data fetches
- [ ] All interactive elements have visible hover and focus states
- [ ] Verified visually using Playwright MCP screenshots against Docker integration data

**Technical Notes:**

- Use Tailwind's consistent spacing scale (p-2, p-4, p-6, etc.) — avoid arbitrary values
- shadcn/ui components already provide good defaults — focus on gaps between components
- Use `transition-all` sparingly; prefer `transition-opacity` or `transition-transform` for performance
- Playwright screenshots at each stage for before/after comparison

---

### Story 15.5: Historical Data Hydration for Accounts & Channels

**Goal:** Ensure the Accounts tab displays peer account balances and payment channel state from historical events, not just live WebSocket data — so the Explorer works correctly when opened after events have already occurred.

**Scope:**

- Add a REST API endpoint (`GET /api/accounts/events`) that returns account/channel-related telemetry events for client-side state reconstruction
- Modify `useAccountBalances` hook to hydrate from historical events on mount before connecting WebSocket
- Modify `usePaymentChannels` hook to hydrate from historical events on mount before connecting WebSocket
- Update `AccountsView` to handle `hydrating` loading state with skeleton loaders
- Verify against Docker integration test: Accounts tab shows peer accounts and payment channels after test completes

**Acceptance Criteria:**

- [ ] Accounts tab shows peer account balances derived from historical events when opening Explorer after events occurred
- [ ] Payment channel cards appear showing all channels (EVM and XRP) reconstructed from historical events
- [ ] Summary stats (Total Accounts, Near Threshold, Active Channels) are accurate and non-zero after hydration
- [ ] WebSocket events after hydration merge correctly into existing state (no duplicates)
- [ ] Wallet overview panel continues to work as before (no regression)
- [ ] Verified against Docker integration test data: ≥1 peer account and ≥1 payment channel visible

**Technical Notes:**

- Uses event replay pattern: fetch historical events from EventStore and replay through existing hook reducer functions
- Reuses existing `applyAccountBalanceEvent`, `applyAgentPaymentEvent`, and `applyChannelEvent` functions
- Account balance events: `ACCOUNT_BALANCE`, `AGENT_CHANNEL_PAYMENT_SENT`
- Channel events (9 types): `PAYMENT_CHANNEL_OPENED`, `PAYMENT_CHANNEL_BALANCE_UPDATE`, `PAYMENT_CHANNEL_SETTLED`, `XRP_CHANNEL_OPENED`, `XRP_CHANNEL_CLAIMED`, `XRP_CHANNEL_CLOSED`, `AGENT_CHANNEL_OPENED`, `AGENT_CHANNEL_BALANCE_UPDATE`, `AGENT_CHANNEL_CLOSED`
- The existing `/api/events` endpoint supports `?types=` filtering; new endpoint is a convenience wrapper

---

### Story 15.6: Peers & Routing Table View

**Goal:** Add a Peers tab to the Connector Explorer showing connected peers with their ILP addresses, on-chain addresses, BTP status, and routing table entries — enabling operators to understand network topology.

**Scope:**

- Add REST API endpoints for peer data (`GET /api/peers`) and routing table (`GET /api/routes`)
- Create `usePeers` and `useRoutingTable` hooks with REST polling
- Create `PeersView` component with peer cards grid and routing table
- Add "Peers" tab to App.tsx navigation alongside Events and Accounts
- Add keyboard shortcut `3` for Peers tab
- Verify against Docker integration test: ≥2 peers with ILP addresses and ≥2 routing entries visible

**Acceptance Criteria:**

- [ ] "Peers" tab appears in Connector Explorer navigation
- [ ] Peer cards display: peer ID, ILP address, EVM address, XRP address, BTP connection status
- [ ] Routing table section shows prefix, next hop peer, and priority for all entries
- [ ] Peer data fetched from REST API on mount and refreshed periodically
- [ ] Routing table next-hop links reference peer cards
- [ ] Keyboard shortcut `3` switches to Peers tab
- [ ] Verified against Docker integration test data: ≥2 peers and ≥2 routing entries

**Technical Notes:**

- Peer data assembled from: `AccountManager.getAccountPairs()`, `AgentNode` follows list, `Connector.getPeers()` (BTP connected)
- Routing data from `ConnectorNode.getRoutes()` — longest-prefix matching per ILP RFC-0027
- `ExplorerServer` constructor already receives `ConnectorNode` instance for backend access
- Follow `useWalletBalances` pattern for REST polling hooks (10s for peers, 30s for routes)
- Use shadcn/ui `Card`, `Badge`, `Table` components
- Wrap `PeersView` with `React.memo`

---

## Compatibility Requirements

- [x] Existing connector APIs remain unchanged
- [x] WebSocket protocol unchanged (backward compatible)
- [x] Database schema unchanged
- [x] Explorer server configuration unchanged (same env vars, same ports)
- [x] All existing Docker Compose topologies continue working
- [x] No changes to backend `explorer-server.ts` required (frontend-only for Stories 15.1, 15.3, 15.4; minor backend batching possible in 15.2)
- [ ] New REST endpoints in Stories 15.5 and 15.6 are additive (no breaking changes to existing API surface)

## Risk Mitigation

- **Primary Risk:** Performance optimizations introduce subtle rendering bugs (missed updates, stale data)
- **Mitigation:** All changes verified against real Docker integration test data; visual regression testing with Playwright screenshots
- **Rollback Plan:** Each story is independently deployable; revert individual PR if issues found

- **Secondary Risk:** Keyboard shortcuts conflict with browser or OS shortcuts
- **Mitigation:** Use standard patterns (vim-style j/k, Escape to close) and only activate when no input is focused
- **Rollback Plan:** Remove keyboard handler without affecting other functionality

## Definition of Done

- [ ] All 6 stories completed with acceptance criteria met
- [ ] "Connector Explorer" branding applied consistently (zero "Explorer" references)
- [ ] Performance benchmarks documented (60fps @ 1000 events, <100ms WebSocket latency)
- [ ] All views verified against Docker integration test data (real ILP packets, settlements, channels)
- [ ] Lighthouse accessibility score ≥ 90
- [ ] Visual consistency audit passed (screenshot comparison)
- [ ] Responsive layout verified at 768px, 1024px, 1440px
- [ ] Keyboard shortcuts documented and functional
- [ ] No regression in existing Explorer functionality
- [ ] Accounts tab populates from historical events (not WebSocket-only)
- [ ] Peers tab shows connected peers with ILP addresses and routing table

## Dependencies

- **Epic 11**: Packet/Event Explorer UI (completed — provides the baseline)

## Technical References

- **Explorer UI Source:** `packages/connector/explorer-ui/src/`
- **Explorer Backend:** `packages/connector/src/explorer/`
- **Docker Integration Test:** `packages/connector/test/integration/`
- **Telemetry Types:** `packages/shared/src/types/telemetry.ts`
- **shadcn/ui v4:** Component library (Radix + Tailwind)

---

**Epic Status:** Completed

**Estimated Stories:** 6

**Architecture Reference:** Frontend enhancement to existing Explorer infrastructure (Epic 11)
