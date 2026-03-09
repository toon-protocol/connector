# Explorer UI Options Explained

## Overview

The Explorer UI has two modes for viewing packets and settlement events:

## Option 1: Standalone Mode (Local SQLite)

### How It Works

```
┌──────────────┐
│   Connector  │
│              │
│  ┌────────┐  │
│  │ Event  │  │──┐
│  │ Store  │  │  │ Writes events locally
│  │(SQLite)│  │  │
│  └────────┘  │◀─┘
│              │
│  ┌────────┐  │
│  │Explorer│  │──┐
│  │  UI    │  │  │ Reads from SQLite
│  │ Server │  │  │
│  └────────┘  │◀─┘
│      ↓       │
└──────┼───────┘
       │
   http://localhost:5173
```

### Characteristics

✅ **Advantages:**

- No external dependencies
- Each peer has its own explorer
- Events stored locally in SQLite
- Simple setup - just enable EXPLORER_ENABLED
- No network latency
- Always available (even offline)

❌ **Disadvantages:**

- **Can only view events from that specific peer**
- No centralized monitoring
- Each peer's events are isolated
- Must access each peer's explorer separately
- Events limited to local node only

### Configuration

```env
# .env
EXPLORER_ENABLED=true
EXPLORER_PORT=5173
# NO DASHBOARD_TELEMETRY_URL needed - omit it for standalone mode
```

**Note:** As of the latest version, Explorer UI works in true standalone mode. Simply enable `EXPLORER_ENABLED=true` and omit `DASHBOARD_TELEMETRY_URL` - the explorer will start and serve local events from SQLite without requiring any external services.

**Access:**

- Peer1: http://localhost:5173
- Peer2: http://localhost:5174
- Peer3: http://localhost:5175
- Peer4: http://localhost:5176
- Peer5: http://localhost:5177

### What You See

When viewing Peer3's explorer (localhost:5175), you'll see:

- ✅ Packets received by Peer3
- ✅ Packets forwarded by Peer3
- ✅ Routing decisions made by Peer3
- ✅ Settlement events for Peer3
- ❌ **NOT** packets from Peer1, Peer2, Peer4, or Peer5

---

## Option 2: Telemetry Aggregator (Centralized)

### How It Works

```
┌──────────────┐    ┌──────────────┐    ┌──────────────┐
│   Peer1      │    │   Peer2      │    │   Peer3      │
│              │    │              │    │              │
│  ┌────────┐  │    │  ┌────────┐  │    │  ┌────────┐  │
│  │ Event  │  │    │  │ Event  │  │    │  │ Event  │  │
│  │ Store  │  │    │  │ Store  │  │    │  │ Store  │  │
│  │(SQLite)│  │    │  │(SQLite)│  │    │  │(SQLite)│  │
│  └────────┘  │    │  └────────┘  │    │  └────────┘  │
│              │    │              │    │              │
│  ┌────────┐  │    │  ┌────────┐  │    │  ┌────────┐  │
│  │Telemetry │─┼───▶│  │Telemetry │─┼───▶│  │Telemetry │─┼───▶
│  │ Emitter│  │ WS │  │ Emitter│  │ WS │  │ Emitter│  │ WS
│  └────────┘  │    │  └────────┘  │    │  └────────┘  │
└──────────────┘    └──────────────┘    └──────────────┘
       │                   │                   │
       └───────────────────┴───────────────────┘
                           │
                           ▼
                  ┌─────────────────┐
                  │   Telemetry     │
                  │   Aggregator    │
                  │   (WebSocket)   │
                  │                 │
                  │  ┌──────────┐   │
                  │  │ Combined │   │
                  │  │  Event   │   │
                  │  │  Store   │   │
                  │  └──────────┘   │
                  │                 │
                  │  ┌──────────┐   │
                  │  │ Explorer │   │
                  │  │    UI    │   │
                  │  └──────────┘   │
                  └────────┼────────┘
                           │
                  http://localhost:3000
```

### Characteristics

✅ **Advantages:**

- **See ALL events from ALL peers in one place**
- Centralized monitoring
- Correlate events across peers
- Track multi-hop packet flow end-to-end
- Single dashboard for entire network
- Historical analysis across all nodes

❌ **Disadvantages:**

- Requires separate service deployment
- WebSocket connections (more complex)
- Single point of failure
- Network overhead (all events sent to aggregator)
- More infrastructure to maintain

### Configuration

```yaml
# docker-compose-monitoring.yml
services:
  telemetry-aggregator:
    image: telemetry-server
    ports:
      - '6001:6001' # WebSocket
      - '3000:3000' # Explorer UI
```

```env
# Each peer's .env
DASHBOARD_TELEMETRY_URL=ws://telemetry-aggregator:6001
EXPLORER_ENABLED=true
```

### What You See

When viewing the centralized explorer (localhost:3000), you'll see:

- ✅ **ALL packets from ALL 5 peers**
- ✅ Complete multi-hop packet traces
- ✅ Settlement events from all peers
- ✅ Claim exchange between peers (Epic 17)
- ✅ Network-wide statistics

---

## Comparison Table

| Feature                | Standalone Mode           | Telemetry Aggregator      |
| ---------------------- | ------------------------- | ------------------------- |
| **Setup Complexity**   | Simple (just enable flag) | Complex (deploy service)  |
| **Visibility**         | Single peer only          | All peers                 |
| **Multi-hop Tracing**  | ❌ No                     | ✅ Yes                    |
| **Infrastructure**     | None (built-in)           | Separate service required |
| **Network Overhead**   | None                      | WebSocket streams         |
| **Use Case**           | Single-node debugging     | Multi-node monitoring     |
| **Packet Correlation** | Local only                | Cross-peer correlation    |

## Recommendation

### For Your Multi-Hop Testing: **Telemetry Aggregator**

Since you want to **view packets traversing all 5 hops**, you need the **Telemetry Aggregator** because:

1. ✅ See the packet journey: Peer1 → Peer2 → Peer3 → Peer4 → Peer5
2. ✅ Correlate PREPARE and FULFILL across all hops
3. ✅ Monitor claim exchange between peers (Epic 17)
4. ✅ Track settlement end-to-end

With standalone mode, you'd only see each peer's local view - you couldn't trace a packet through all 5 hops.

## Quick Decision Matrix

**Want to see:**

- ❓ "What packets did Peer3 handle?" → Standalone mode
- ❓ "How did this packet traverse all 5 peers?" → **Telemetry Aggregator** ✅
- ❓ "What's happening across my network?" → **Telemetry Aggregator** ✅
- ❓ "Debug a single peer's behavior?" → Standalone mode

**For multi-hop packet visualization, you need Telemetry Aggregator.**
