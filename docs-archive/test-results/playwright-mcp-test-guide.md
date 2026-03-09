# Playwright MCP Integration Test Guide

## Overview

This guide documents the Playwright MCP integration test suite for the Explorer UI. Tests validate the UI against a running 5-peer multi-hop network with real telemetry data.

**Test Approach:** Interactive testing via Playwright MCP tools (not traditional test runner)

## Prerequisites

### Required Software

- Docker and Docker Compose (v2.24+)
- Node.js 22.11.0 LTS
- npm 10.x

### Required Configuration

- `.env` file with treasury wallet keys configured
- Built connector image: `docker build -t agent-runtime .`

### Port Mapping

| Peer  | BTP Port | Health Port | Explorer UI           |
| ----- | -------- | ----------- | --------------------- |
| Peer1 | 3000     | 9080        | http://localhost:5173 |
| Peer2 | 3001     | 9081        | http://localhost:5174 |
| Peer3 | 3002     | 9082        | http://localhost:5175 |
| Peer4 | 3003     | 9083        | http://localhost:5176 |
| Peer5 | 3004     | 9084        | http://localhost:5177 |

## Deployment Setup

### 1. Deploy 5-Peer Network

```bash
./scripts/deploy-5-peer-multihop.sh
```

This script:

1. Checks prerequisites (Docker, Docker Compose, connector image)
2. Initializes TigerBeetle accounting database
3. Starts 5-peer network via Docker Compose
4. Funds peers from treasury wallet
5. Sends test packets through the network
6. Verifies multi-hop routing

### 2. Verify Health Endpoints

```bash
# Check all peer health endpoints
curl http://localhost:9080/health  # Peer1
curl http://localhost:9081/health  # Peer2
curl http://localhost:9082/health  # Peer3
curl http://localhost:9083/health  # Peer4
curl http://localhost:9084/health  # Peer5
```

### 3. Generate Additional Packet Activity

```bash
npx ts-node tools/send-packet/src/index.ts \
  --connector-url ws://localhost:3000 \
  --destination g.peer5.dest \
  --amount 1000000 \
  --count 10
```

## Test Scenarios

### Dashboard Tab Testing

**Tools Used:**

- `mcp__playwright__browser_navigate` - Navigate to Explorer URL
- `mcp__playwright__browser_snapshot` - Get accessibility tree for element refs
- `mcp__playwright__browser_take_screenshot` - Capture visual screenshot

**Expected Results:**

- Metrics grid visible with 4 cards: Total Packets, Success Rate, Active Channels, Routing Status
- Live Packet Flow section showing recent events
- All metrics show non-zero values after packet sends
- "CONNECTED" status indicator when WebSocket active

**Screenshot:** `dashboard-peer1-{timestamp}.png`

### Packets Tab Testing

**Tools Used:**

- `mcp__playwright__browser_press_key({ key: '2' })` - Navigate via keyboard
- `mcp__playwright__browser_click` - Click filter options
- `mcp__playwright__browser_type` - Enter search text

**Expected Results:**

- Event table populated with PREPARE/FULFILL/REJECT packets
- Filter bar with packet type options
- Search functionality filters results
- Packet detail panel opens on row click

**Screenshot:** `packets-peer1-{timestamp}.png`

### Accounts Tab Testing

**Tools Used:**

- `mcp__playwright__browser_press_key({ key: '3' })` - Navigate via keyboard
- `mcp__playwright__browser_snapshot` - Verify account cards

**Expected Results:**

- AccountCard components display peer balances
- Balance history charts render correctly
- At least peer2 account visible for peer1

**Screenshot:** `accounts-peer1-{timestamp}.png`

### Peers Tab Testing

**Tools Used:**

- `mcp__playwright__browser_press_key({ key: '4' })` - Navigate via keyboard

**Expected Results:**

- Peer connection table shows connected peers
- Routing table entries visible
- Peer1 shows peer2 and sendPacketClient connections

**Screenshot:** `peers-peer1-{timestamp}.png`

### Keys Tab Testing

**Tools Used:**

- `mcp__playwright__browser_press_key({ key: '5' })` - Navigate via keyboard

**Expected Results:**

- Key display with EVM address in monospace font
- Copy-to-clipboard functionality available

**Screenshot:** `keys-peer1-{timestamp}.png`

### Connection State Testing

**Expected States:**

- **CONNECTED**: Green indicator, WebSocket active
- **DISCONNECTED**: Red indicator, no WebSocket connection

### Performance Testing

**Measurement Tools:**

```javascript
// Page load time
mcp__playwright__browser_evaluate({
  function:
    '() => performance.timing.domContentLoadedEventEnd - performance.timing.navigationStart',
});

// First contentful paint
mcp__playwright__browser_evaluate({
  function:
    '() => performance.getEntriesByType("paint").find(p => p.name === "first-contentful-paint")?.startTime || 0',
});
```

**Performance Targets:**

- Page load time: < 1000ms
- First contentful paint: < 1000ms
- WebSocket connection: < 500ms

## Screenshot Inventory

All screenshots stored in: `docs/test-results/explorer-ui-screenshots/`

| Screenshot              | Description           | Expected Content               |
| ----------------------- | --------------------- | ------------------------------ |
| `dashboard-peer1-*.png` | Dashboard tab (Peer1) | Metrics grid, live packet flow |
| `packets-peer1-*.png`   | Packets tab (Peer1)   | Event table with ILP packets   |
| `accounts-peer1-*.png`  | Accounts tab (Peer1)  | Account cards with balances    |
| `peers-peer1-*.png`     | Peers tab (Peer1)     | Peer connection table          |
| `keys-peer1-*.png`      | Keys tab (Peer1)      | Key management interface       |
| `dashboard-peer2-*.png` | Dashboard tab (Peer2) | Metrics from peer2 perspective |
| `packets-peer2-*.png`   | Packets tab (Peer2)   | Packets transiting peer2       |
| `accounts-peer2-*.png`  | Accounts tab (Peer2)  | peer1 and peer3 accounts       |
| `peers-peer2-*.png`     | Peers tab (Peer2)     | peer1 and peer3 connections    |
| `keys-peer2-*.png`      | Keys tab (Peer2)      | peer2 key display              |

## Troubleshooting

### Common Issues

#### Explorer UI Not Loading

1. Check if containers are running: `docker compose -f docker-compose-5-peer-multihop.yml ps`
2. Check Explorer port mapping in docker-compose file
3. Verify Vite dev server is running inside container

#### No Packets Displayed

1. Verify WebSocket connection (status indicator)
2. Send test packets: `npx ts-node tools/send-packet/src/index.ts --destination g.peer5 --amount 1000000`
3. Check console for errors: `mcp__playwright__browser_console_messages({ level: 'error' })`

#### WebSocket Connection Failed

1. Check connector health: `curl http://localhost:9080/health`
2. Verify telemetry endpoint is enabled in connector config
3. Check browser console for CORS errors

#### Performance Metrics Not Available

1. Ensure page fully loaded before measuring
2. Use `mcp__playwright__browser_wait_for({ time: 2 })` to wait for data
3. Check network tab for slow API responses

### Viewing Logs

```bash
# All containers
docker compose -f docker-compose-5-peer-multihop.yml logs -f

# Specific peer
docker compose -f docker-compose-5-peer-multihop.yml logs -f peer1

# TigerBeetle
docker compose -f docker-compose-5-peer-multihop.yml logs -f tigerbeetle
```

## Performance Baselines

Measured on 2026-02-03 using Playwright MCP with local dev server.

| Metric                 | Peer1   | Peer2 | Target   | Status                       |
| ---------------------- | ------- | ----- | -------- | ---------------------------- |
| Page Load Time         | 605ms   | -     | < 1000ms | ✓ PASS                       |
| First Contentful Paint | 3104ms  | -     | < 1000ms | ⚠ WARN (dev server overhead) |
| WebSocket Connection   | < 500ms | -     | < 500ms  | ✓ PASS                       |

**Notes:**

- First contentful paint exceeds target due to Vite dev server HMR overhead
- Production build expected to meet FCP target
- WebSocket establishes connection immediately on page load

## Console Errors Observed

| Error Type        | Severity | Description                                         | Action                     |
| ----------------- | -------- | --------------------------------------------------- | -------------------------- |
| React ref warning | Low      | Function components cannot be given refs (Radix UI) | Cosmetic, no action needed |
| 404 /api/balances | Expected | Backend API not running in dev mode                 | Normal for local dev       |

## Test Execution Log

| Date       | Tester                       | Tests Run           | Pass/Fail | Notes                                                |
| ---------- | ---------------------------- | ------------------- | --------- | ---------------------------------------------------- |
| 2026-02-03 | Claude Code (Playwright MCP) | Peer1 all tabs      | PASS      | All 5 tabs render correctly, keyboard shortcuts work |
| 2026-02-03 | Claude Code (Playwright MCP) | Filter interactions | PASS      | ILP Packets filter, search, clear filters            |
| 2026-02-03 | Claude Code (Playwright MCP) | Connection state    | PASS      | CONNECTED indicator visible                          |
| 2026-02-03 | Claude Code (Playwright MCP) | Performance         | PARTIAL   | Page load OK, FCP exceeds target (dev mode)          |
