# Explorer UI Developer Guide

> Technical guide for developing and extending the Explorer UI

This guide covers component architecture, data flow, customization, testing, and deployment of the Explorer UI.

## Table of Contents

- [Component Architecture](#component-architecture)
- [Data Flow](#data-flow)
- [Adding New Metrics](#adding-new-metrics)
- [Customizing the Theme](#customizing-the-theme)
- [Performance Optimization](#performance-optimization)
- [Testing Patterns](#testing-patterns)
- [Deployment Guide](#deployment-guide)

---

## Component Architecture

### Directory Structure

```
packages/connector/explorer-ui/
├── src/
│   ├── App.tsx                    # Main application with tab routing
│   ├── main.tsx                   # React entry point
│   ├── index.css                  # Tailwind + NOC theme + animations
│   ├── components/
│   │   ├── Dashboard.tsx          # Dashboard tab with metrics grid
│   │   ├── EventTable.tsx         # Packets tab with event streaming
│   │   ├── AccountsView.tsx       # Accounts tab with balance cards
│   │   ├── PeersView.tsx          # Peers tab with connection table
│   │   ├── KeyManager.tsx         # Keys tab with key management
│   │   ├── Header.tsx             # Header with branding and status
│   │   ├── FilterBar.tsx          # Filter controls for Packets tab
│   │   ├── AccountCard.tsx        # Individual account display
│   │   ├── BalanceHistoryChart.tsx # Balance trend visualization
│   │   ├── SettlementTimeline.tsx # Settlement event timeline
│   │   └── ui/                    # shadcn/ui components
│   │       ├── button.tsx
│   │       ├── card.tsx
│   │       ├── table.tsx
│   │       └── ...
│   ├── hooks/
│   │   ├── useEventStream.ts      # WebSocket connection hook
│   │   └── useEventStream.test.ts
│   └── lib/
│       ├── event-types.ts         # Frontend telemetry types
│       └── utils.ts               # shadcn cn() helper
├── index.html
├── vite.config.ts
├── tailwind.config.js
├── tsconfig.json
└── package.json
```

### Key Components

#### App.tsx

Main application component managing:

- Tab state (`activeTab`)
- Keyboard shortcut listeners
- Layout structure

```tsx
// Tab navigation
const [activeTab, setActiveTab] = useState('dashboard');

// Keyboard shortcuts
useEffect(() => {
  const handleKeyPress = (e: KeyboardEvent) => {
    if (e.key >= '1' && e.key <= '5') {
      const tabs = ['dashboard', 'packets', 'accounts', 'peers', 'keys'];
      setActiveTab(tabs[parseInt(e.key) - 1]);
    }
  };
  window.addEventListener('keypress', handleKeyPress);
  return () => window.removeEventListener('keypress', handleKeyPress);
}, []);
```

#### Header.tsx

Header component displaying:

- Logo and branding
- WebSocket connection status
- Tab navigation

#### useEventStream.ts

Custom hook for WebSocket connection:

- Auto-reconnection logic
- Event buffering
- Connection state management

```tsx
const { events, isConnected, error } = useEventStream({
  url: 'ws://localhost:3001/events',
  maxEvents: 1000,
});
```

---

## Data Flow

### Telemetry Pipeline

```
┌─────────────────────────────────────────────────────────────────┐
│ Backend (packages/connector/src)                                 │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ConnectorNode                                                   │
│       │                                                          │
│       ▼                                                          │
│  TelemetryEmitter.emit(event)                                    │
│       │                                                          │
│       ▼                                                          │
│  TelemetryBuffer (batching, 100ms window)                        │
│       │                                                          │
│       ▼                                                          │
│  EventStore.insertEvents() ──► libSQL (persistence)              │
│       │                                                          │
│       ▼                                                          │
│  EventBroadcaster.broadcast()                                    │
│       │                                                          │
│       ▼                                                          │
│  WebSocket Server (port 3001)                                    │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
                              │
                              │ WebSocket
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│ Frontend (packages/connector/explorer-ui/src)                    │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  useEventStream hook                                             │
│       │                                                          │
│       │ WebSocket.onmessage                                      │
│       ▼                                                          │
│  Event[] state (React.useState)                                  │
│       │                                                          │
│       │ props                                                    │
│       ▼                                                          │
│  Component renders (Dashboard, EventTable, etc.)                 │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### Event Types

Events flow from backend to frontend with these types:

```typescript
// packages/shared/src/types/telemetry.ts
interface TelemetryEvent {
  id: string;
  timestamp: number;
  type: 'packet' | 'settlement' | 'channel' | 'peer';
  data: PacketEvent | SettlementEvent | ChannelEvent | PeerEvent;
}

interface PacketEvent {
  packetType: 'PREPARE' | 'FULFILL' | 'REJECT';
  amount?: string;
  sourceAddress: string;
  destinationAddress: string;
  executionCondition?: string;
  fulfillment?: string;
  errorCode?: string;
}
```

---

## Adding New Metrics

### Step 1: Define the Metric

Add metric calculation to Dashboard.tsx:

```tsx
// Dashboard.tsx
const calculateNewMetric = (events: TelemetryEvent[]): number => {
  return events.filter(e => /* your logic */).length;
};
```

### Step 2: Add to Metrics Grid

Add a new metric card:

```tsx
// In Dashboard.tsx metrics grid
<Card className="fade-in-up stagger-5">
  <CardHeader>
    <CardTitle className="text-sm text-muted-foreground">New Metric</CardTitle>
  </CardHeader>
  <CardContent>
    <div className="text-2xl font-bold text-cyan-400">{calculateNewMetric(events)}</div>
  </CardContent>
</Card>
```

### Step 3: Update Grid Layout

Adjust Tailwind grid classes if needed:

```tsx
<div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-5 gap-4">{/* 5 metrics now */}</div>
```

### Step 4: Add Tests

```typescript
// Dashboard.test.tsx
it('calculates new metric correctly', () => {
  const events = [/* test events */];
  render(<Dashboard events={events} />);
  expect(screen.getByText('Expected Value')).toBeInTheDocument();
});
```

---

## Customizing the Theme

### Color Variables

Modify `src/index.css` to change the color scheme:

```css
:root {
  /* Background colors */
  --background: 222 47% 6%; /* Deep space */
  --card: 222 47% 8%; /* Cards */
  --muted: 217 33% 17%; /* Muted backgrounds */

  /* Text colors */
  --foreground: 210 40% 98%; /* Primary text */
  --muted-foreground: 215 20% 65%; /* Secondary text */

  /* Accent colors */
  --primary: 199 89% 48%; /* Cyan */
  --destructive: 0 84% 60%; /* Rose */
}
```

### Creating a Light Theme

Add a `.light` class variant:

```css
.light {
  --background: 0 0% 100%;
  --foreground: 222 47% 6%;
  --card: 0 0% 98%;
  /* ... */
}
```

Toggle with JavaScript:

```tsx
document.documentElement.classList.toggle('light');
```

### Custom Accent Colors

To change the packet type colors:

```tsx
// Instead of hardcoded colors
const packetColors = {
  PREPARE: 'text-cyan-400',
  FULFILL: 'text-emerald-400',
  REJECT: 'text-rose-400',
};

// Use CSS variables
const packetColors = {
  PREPARE: 'text-[hsl(var(--packet-prepare))]',
  FULFILL: 'text-[hsl(var(--packet-fulfill))]',
  REJECT: 'text-[hsl(var(--packet-reject))]',
};
```

---

## Performance Optimization

### Event Limiting

Limit stored events to prevent memory issues:

```tsx
// useEventStream.ts
const MAX_EVENTS = 1000;

const addEvent = (event: TelemetryEvent) => {
  setEvents((prev) => {
    const updated = [event, ...prev];
    return updated.slice(0, MAX_EVENTS);
  });
};
```

### React.memo for Lists

Memoize list items to prevent unnecessary re-renders:

```tsx
const EventRow = React.memo(({ event }: { event: TelemetryEvent }) => {
  return <TableRow>{/* ... */}</TableRow>;
});
```

### Virtual Scrolling

For large event lists, implement virtual scrolling:

```tsx
import { useVirtualizer } from '@tanstack/react-virtual';

const rowVirtualizer = useVirtualizer({
  count: events.length,
  getScrollElement: () => parentRef.current,
  estimateSize: () => 48, // row height
});
```

### Animation Performance

Disable animations for performance:

```tsx
// Check user preference
const prefersReducedMotion = window.matchMedia(
  '(prefers-reduced-motion: reduce)'
).matches;

// Conditionally apply animation classes
<div className={prefersReducedMotion ? '' : 'fade-in-up'}>
```

---

## Testing Patterns

### Component Tests

Using Vitest and React Testing Library:

```tsx
// Dashboard.test.tsx
import { render, screen } from '@testing-library/react';
import { describe, it, expect } from 'vitest';
import Dashboard from './Dashboard';

describe('Dashboard', () => {
  it('renders metrics grid', () => {
    render(<Dashboard events={[]} isConnected={true} />);
    expect(screen.getByText('Total Packets')).toBeInTheDocument();
    expect(screen.getByText('Success Rate')).toBeInTheDocument();
  });

  it('calculates success rate correctly', () => {
    const events = [
      { type: 'packet', data: { packetType: 'FULFILL' } },
      { type: 'packet', data: { packetType: 'FULFILL' } },
      { type: 'packet', data: { packetType: 'REJECT' } },
    ];
    render(<Dashboard events={events} isConnected={true} />);
    expect(screen.getByText('66.7%')).toBeInTheDocument();
  });
});
```

### Hook Tests

Testing custom hooks:

```tsx
// useEventStream.test.ts
import { renderHook, act } from '@testing-library/react';
import { useEventStream } from './useEventStream';

describe('useEventStream', () => {
  it('connects to WebSocket', async () => {
    const { result } = renderHook(() => useEventStream({ url: 'ws://localhost:3001/events' }));

    await waitFor(() => {
      expect(result.current.isConnected).toBe(true);
    });
  });
});
```

### Playwright E2E Tests

See `docs/test-results/playwright-mcp-test-guide.md` for comprehensive Playwright testing guide.

```typescript
// Basic E2E test structure
test('Dashboard loads correctly', async ({ page }) => {
  await page.goto('http://localhost:5173');
  await expect(page.getByText('Dashboard')).toBeVisible();
  await expect(page.getByText('Total Packets')).toBeVisible();
});
```

---

## Deployment Guide

### Development Server

```bash
cd packages/connector/explorer-ui
npm install
npm run dev
```

Opens at http://localhost:5173 with hot reload.

### Production Build

```bash
npm run build
```

Outputs to `dist/` directory. The build:

- Minifies JavaScript/CSS
- Tree-shakes unused code
- Generates source maps
- Optimizes assets

### Docker Deployment

The Explorer UI is served by the connector container. Port mapping:

| Peer  | BTP Port | Health Port | Explorer UI           |
| ----- | -------- | ----------- | --------------------- |
| Peer1 | 3000     | 9080        | http://localhost:5173 |
| Peer2 | 3001     | 9081        | http://localhost:5174 |
| Peer3 | 3002     | 9082        | http://localhost:5175 |
| Peer4 | 3003     | 9083        | http://localhost:5176 |
| Peer5 | 3004     | 9084        | http://localhost:5177 |

### Environment Variables

| Variable       | Default               | Description        |
| -------------- | --------------------- | ------------------ |
| `VITE_WS_URL`  | `ws://localhost:3001` | WebSocket endpoint |
| `VITE_API_URL` | `/api`                | REST API base URL  |

Set in `.env.local` or Docker Compose:

```yaml
# docker-compose.yml
services:
  peer1:
    environment:
      - EXPLORER_PORT=5173
      - EXPLORER_WS_URL=ws://localhost:3001
```

### Multi-Node Deployment

For multi-node deployments, each node runs its own Explorer on a unique port:

```yaml
# docker-compose.yml
services:
  peer1:
    ports:
      - '5173:5173' # Explorer UI
      - '3000:3000' # BTP
      - '9080:9080' # Health

  peer2:
    ports:
      - '5174:5173' # Explorer UI (different host port)
      - '3001:3000' # BTP
      - '9081:9080' # Health
```

### Nginx Reverse Proxy

For production deployments behind a reverse proxy:

```nginx
server {
    listen 80;
    server_name explorer.example.com;

    location / {
        proxy_pass http://localhost:5173;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
    }

    location /ws {
        proxy_pass http://localhost:3001;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
    }
}
```
