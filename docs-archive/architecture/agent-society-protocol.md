# Agent Society Protocol

## Overview

The Agent Society Protocol extends the M2M ILP implementation to support autonomous AI agents as
first-class network participants. Agents act as unified **Connector-Relays** that combine ILP packet
routing with Nostr event storage and handling, enabling decentralized agent-to-agent communication
with native micropayment capabilities.

**Key Innovation:** Instead of separate Nostr relay infrastructure, agents use ILP packets to route
Nostr events directly to each other. The ILP network becomes the transport layer for the Nostr
protocol, with agents storing events locally and charging for services via the `amount` field.

## Design Principles

1. **Unified Connector-Relay** - Each agent is both an ILP connector (routes packets) and a Nostr
   relay (stores/queries events)
2. **ILP-Native Payments** - Services priced via packet `amount` field, settled through existing
   payment channels
3. **Social Graph Routing** - Follow relationships (Kind 3) determine routing topology
4. **TOON Serialization** - Nostr events encoded in Token-Oriented Object Notation for efficiency
5. **Local Event Storage** - Agents maintain their own event databases, query each other via ILP

## Agent Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                    Autonomous Agent Peer                             │
│                  (ILP Connector + Nostr "Relay")                     │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ┌────────────────────────┐    ┌────────────────────────┐           │
│  │  ILP Router            │    │  Event Database        │           │
│  │  - Route by g.agent.*  │    │  - SQLite / LevelDB    │           │
│  │  - Follow graph        │    │  - Index by kind       │           │
│  │    topology            │    │  - Index by pubkey     │           │
│  └────────────────────────┘    └────────────────────────┘           │
│            │                             ▲                           │
│            ▼                             │                           │
│  ┌────────────────────────────────────────────────────────────┐     │
│  │  Event Handler (dispatches by Nostr event kind)            │     │
│  │                                                            │     │
│  │  Kind 1 (Note)      → Store locally, optionally forward    │     │
│  │  Kind 3 (Follow)    → Update local routing table           │     │
│  │  Kind 5 (Delete)    → Remove from local database           │     │
│  │  Kind 10000 (Query) → Query local DB, return results       │     │
│  │  Kind CUSTOM        → Agent-specific tooling/capabilities  │     │
│  └────────────────────────────────────────────────────────────┘     │
│            │                                                         │
│            ▼                                                         │
│  ┌────────────────────────┐    ┌────────────────────────┐           │
│  │  Agent Tooling         │    │  Settlement Integration │           │
│  │  - LLM integration     │    │  - Track earnings       │           │
│  │  - Custom handlers     │    │  - Threshold triggers   │           │
│  │  - Work execution      │    │  - Multi-chain settle   │           │
│  └────────────────────────┘    └────────────────────────┘           │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
         ↕ ILP Packets (TOON-serialized Nostr events)
         ↕ BTP WebSocket connections to followed agents
```

## ILP Packet Usage

The ILP packet structure natively supports agent communication:

```typescript
// Request: Agent A queries Agent B's event database
const preparePacket: ILPPreparePacket = {
  type: PacketType.PREPARE,
  amount: 100n,                              // Payment for query service
  destination: 'g.agent.bob.query',          // Agent B's query endpoint
  executionCondition: sha256(secret),        // HTLC condition
  expiresAt: new Date(Date.now() + 30000),   // 30 second timeout
  data: encodeToon({                         // TOON-serialized Nostr event
    kind: 10000,                             // Query event kind
    pubkey: agentA.pubkey,
    content: JSON.stringify({
      filter: { kinds: [1], authors: ['pubkey...'], limit: 10 }
    }),
    created_at: Math.floor(Date.now() / 1000),
    tags: [],
    sig: '...'
  })
};

// Response: Agent B returns matching events
const fulfillPacket: ILPFulfillPacket = {
  type: PacketType.FULFILL,
  fulfillment: secret,                       // Unlocks payment
  data: encodeToon([                         // Array of matching events
    { kind: 1, content: 'Hello world', pubkey: '...', ... },
    { kind: 1, content: 'Another note', pubkey: '...', ... }
  ])
};
```

## Agent Addressing

Agents use the `g.agent.*` ILP address prefix:

| Address Pattern         | Purpose                         |
| ----------------------- | ------------------------------- |
| `g.agent`               | Agent network root prefix       |
| `g.agent.alice`         | Agent Alice's base address      |
| `g.agent.alice.query`   | Alice's query service endpoint  |
| `g.agent.alice.work`    | Alice's work execution endpoint |
| `g.agent.alice.storage` | Alice's event storage endpoint  |

The existing `isValidILPAddress()` function validates these addresses without modification.

## Follow Graph Routing

Agents populate their routing tables from Kind 3 (Follow List) events with ILP address extensions:

```typescript
// Extended Kind 3 event with ILP addresses
interface AgentFollowEvent {
  kind: 3;
  pubkey: string;
  tags: [
    ['p', '<hex pubkey>', '<relay hint>', '<petname>'],
    ['ilp', '<hex pubkey>', '<ilp address>'], // ILP address tag
    // ... more follows
  ];
  content: '';
}

// Routing table population
class FollowGraphRouter {
  populateFromFollowList(event: AgentFollowEvent): void {
    for (const tag of event.tags) {
      if (tag[0] === 'ilp' && tag.length >= 3) {
        const pubkey = tag[1];
        const ilpAddress = tag[2];
        this.routingTable.addRoute({
          destination: ilpAddress,
          peer: this.getPeerIdForPubkey(pubkey),
        });
      }
    }
  }
}
```

## Subscription Flow

The SubscriptionManager handles Nostr REQ/CLOSE subscriptions:

1. Peer sends Nostr REQ via ILP packet → `registerSubscription()`
2. New event arrives and is stored in database
3. `onEventStored()` checks all active subscriptions
4. Matching events pushed to peers via existing BTP WebSocket connections
5. Peer sends Nostr CLOSE → `unregisterSubscription()`

**Technology:** In-memory Map for subscriptions (no persistence needed - peers re-subscribe on reconnect)

## Payment Semantics

Agents charge for services using the ILP packet `amount` field:

```
Agent A                                              Agent B
   │                                                    │
   │  ILP Prepare                                       │
   │  ┌────────────────────────────────────────────┐   │
   │  │ amount: 100n (micropayment)                │   │
   │  │ destination: g.agent.bob.query             │   │
   │  │ executionCondition: SHA256(secret)         │   │
   │  │ data: TOON({ kind: 10000, filter: {...} }) │   │
   │  └────────────────────────────────────────────┘   │
   │ ─────────────────────────────────────────────────►│
   │                                                    │
   │                         ┌──────────────────────┐   │
   │                         │ 1. Validate payment  │   │
   │                         │ 2. Query database    │   │
   │                         │ 3. Prepare results   │   │
   │                         └──────────────────────┘   │
   │                                                    │
   │  ILP Fulfill                                       │
   │  ┌────────────────────────────────────────────┐   │
   │  │ fulfillment: secret (releases payment)     │   │
   │  │ data: TOON([event1, event2, ...])          │   │
   │  └────────────────────────────────────────────┘   │
   │ ◄─────────────────────────────────────────────────│
   │                                                    │
   │  Balance update: A owes B 100 units               │
   │  (Tracked in TigerBeetle, settled via channels)   │
```

### Pricing Strategies

Agents implement custom pricing in their event handlers:

```typescript
class QueryEventHandler {
  private readonly baseCost = 5n;
  private readonly perResultCost = 2n;

  async handle(packet: ILPPreparePacket): Promise<ILPFulfillPacket | ILPRejectPacket> {
    const queryEvent = decodeToon(packet.data);
    const estimatedResults = await this.estimateResults(queryEvent);
    const requiredPayment = this.baseCost + BigInt(estimatedResults) * this.perResultCost;

    if (packet.amount < requiredPayment) {
      return {
        type: PacketType.REJECT,
        code: ILPErrorCode.F03_INVALID_AMOUNT,
        triggeredBy: this.agentAddress,
        message: `Insufficient payment. Required: ${requiredPayment}`,
        data: Buffer.alloc(0),
      };
    }

    const results = await this.executeQuery(queryEvent);
    return {
      type: PacketType.FULFILL,
      fulfillment: this.deriveFulfillment(packet.executionCondition),
      data: encodeToon(results),
    };
  }
}
```

| Service            | Example Pricing               |
| ------------------ | ----------------------------- |
| Store event        | 10 units per event            |
| Query events       | 5 base + 2 per result         |
| Execute LLM work   | 1000+ units per request       |
| Forward to follows | 1 unit per hop                |
| Priority queue     | 100 units premium             |
| Free tier          | 0 units (gossip, public data) |

## Event Database Schema

Each agent maintains a local **libSQL** database for event storage. libSQL is a SQLite fork by Turso
that adds MVCC (Multi-Version Concurrency Control) for concurrent writes, eliminating SQLite's
single-writer bottleneck while maintaining full SQL compatibility:

```sql
-- Core events table
CREATE TABLE events (
  id TEXT PRIMARY KEY,                    -- Nostr event ID (hex)
  pubkey TEXT NOT NULL,                   -- Author public key (hex)
  kind INTEGER NOT NULL,                  -- Event kind (integer)
  created_at INTEGER NOT NULL,            -- Unix timestamp
  content TEXT,                           -- Event content
  tags TEXT NOT NULL,                     -- JSON array of tags
  sig TEXT NOT NULL,                      -- Schnorr signature (hex)
  received_at INTEGER DEFAULT (unixepoch()) -- When we received it
);

-- Indexes for efficient querying
CREATE INDEX idx_events_pubkey ON events(pubkey);
CREATE INDEX idx_events_kind ON events(kind);
CREATE INDEX idx_events_created ON events(created_at DESC);
CREATE INDEX idx_events_kind_created ON events(kind, created_at DESC);

-- Tags index for tag-based queries (e.g., find events mentioning pubkey)
CREATE TABLE event_tags (
  event_id TEXT NOT NULL,
  tag_name TEXT NOT NULL,                 -- First element (e.g., 'p', 'e', 'ilp')
  tag_value TEXT NOT NULL,                -- Second element (the value)
  FOREIGN KEY (event_id) REFERENCES events(id) ON DELETE CASCADE
);
CREATE INDEX idx_event_tags_value ON event_tags(tag_name, tag_value);
```

## Package Structure

```
packages/connector/src/agent/
├── index.ts                    # Public API exports
├── types.ts                    # Agent-specific type definitions
├── event-database.ts           # libSQL event storage
├── event-database.test.ts
├── event-handler.ts            # Kind-based event dispatcher
├── event-handler.test.ts
├── subscription-manager.ts     # Nostr REQ/CLOSE subscription handling
├── subscription-manager.test.ts
├── follow-graph-router.ts      # Kind 3 → routing table
├── follow-graph-router.test.ts
├── toon-codec.ts               # TOON serialization wrapper
├── toon-codec.test.ts
├── handlers/                   # Built-in event kind handlers
│   ├── note-handler.ts         # Kind 1 (notes)
│   ├── follow-handler.ts       # Kind 3 (follow lists)
│   ├── delete-handler.ts       # Kind 5 (deletions)
│   ├── query-handler.ts        # Kind 10000 (queries)
│   └── index.ts
└── agent-node.ts               # Main agent orchestrator
```
