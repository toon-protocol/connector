# CCP: Connector-to-Connector Protocol

## What is CCP?

**CCP (Connector-to-Connector Protocol)** is a proposed ILP protocol for **dynamic route advertisement** between connectors, similar to BGP in traditional IP networks.

## Purpose

CCP enables connectors to:

- **Advertise routes** to neighboring connectors
- **Discover available paths** automatically
- **Update routing tables** dynamically
- **Respond to topology changes** in real-time

## How CCP Works (Conceptual)

### Route Advertisement

```
Connector A                    Connector B
───────────                    ───────────
"I can reach g.alice.*"  ───────▶  Updates routing table
"Cost: 0.1% fee"                   Adds route: g.alice.* → Connector A
"Liquidity: 1000 USD"

                         ◀───────  "I can reach g.bob.*"
Update routing table              "Cost: 0.2% fee"
Adds route: g.bob.* → Connector B
```

### Route Propagation

```
Connector A ────▶ Connector B ────▶ Connector C

Connector A advertises: "g.alice.*"
Connector B receives, adds to table, re-advertises: "g.alice.*" (via B)
Connector C receives, adds to table: "g.alice.* via B"

Result: Connector C knows path to alice through B through A
```

### Dynamic Updates

```
Connector B detects:
- Link to Connector A failed
- OR liquidity exhausted
- OR route cost changed

Connector B broadcasts:
- WITHDRAW route "g.alice.*"
- OR UPDATE route with new cost/liquidity

All neighbors update routing tables automatically
```

## CCP Message Types (Conceptual)

### 1. ROUTE_UPDATE

```json
{
  "type": "ROUTE_UPDATE",
  "routes": [
    {
      "prefix": "g.alice",
      "path": ["connector-b", "connector-a"],
      "cost": 0.001,
      "liquidity": "1000000000"
    }
  ]
}
```

### 2. ROUTE_WITHDRAW

```json
{
  "type": "ROUTE_WITHDRAW",
  "prefixes": ["g.alice", "g.bob.usd"]
}
```

### 3. ROUTE_REQUEST

```json
{
  "type": "ROUTE_REQUEST",
  "destination": "g.alice.wallet.usd"
}
```

## CCP vs Static Routing

### Static Routing (Current M2M Implementation)

```yaml
# Manually configured in YAML
routes:
  - prefix: g.peer1
    nextHop: peer1
  - prefix: g.peer5
    nextHop: peer2 # Manually configured path
```

**Characteristics:**

- ✅ Simple, predictable
- ✅ Full control
- ✅ No protocol overhead
- ❌ Manual configuration
- ❌ No automatic failover
- ❌ No topology adaptation

### Dynamic Routing (CCP)

```typescript
// Automatically learned from CCP advertisements
routingTable.addRoute({
  prefix: 'g.peer5',
  nextHop: 'peer2',
  cost: 0.001,
  learned: true, // From CCP
  ttl: 3600, // Expires if not refreshed
});
```

**Characteristics:**

- ✅ Automatic route discovery
- ✅ Topology adaptation
- ✅ Failover support
- ❌ More complex
- ❌ Protocol overhead
- ❌ Potential routing loops

## CCP Status in ILP Ecosystem

**Current Status:**

- **No official RFC** (CCP is conceptual/proposed)
- **Not widely implemented** in production ILP connectors
- **Most connectors use static routing** (like M2M)
- **Alternative:** Manual route configuration via APIs

**Historical Context:**

- CCP was discussed in early ILP development
- Similar protocols exist (e.g., Routing Table Protocol in some implementations)
- Modern approach: Static config + operational tools for route management

## CCP vs BGP Comparison

### BGP (Border Gateway Protocol)

**Internet routing protocol:**

- Autonomous systems advertise IP prefixes
- Path vector protocol (full AS path)
- Policy-based routing
- Large-scale (global Internet)

### CCP (Connector-to-Connector Protocol)

**ILP routing protocol (proposed):**

- Connectors advertise ILP address prefixes
- Cost-based routing (fees, liquidity)
- Economic routing decisions
- Smaller scale (payment networks)

### Similarities

- Prefix-based routing (longest-prefix matching)
- Route advertisements between neighbors
- Path attributes (cost, path length)
- Withdraw/update mechanisms

### Differences

| Feature              | BGP                      | CCP                   |
| -------------------- | ------------------------ | --------------------- |
| **Routing metric**   | AS path length, policies | Cost, liquidity       |
| **Update frequency** | Seconds to minutes       | Potentially real-time |
| **Loop prevention**  | AS path checking         | ?? (not specified)    |
| **Convergence**      | Eventually consistent    | ?? (not specified)    |
| **Trust model**      | Policy-based filtering   | Economic incentives   |

## Why M2M Uses Static Routing

For a **5-peer linear chain**, static routing is appropriate because:

1. **Topology is fixed** - Peer1 → Peer2 → Peer3 → Peer4 → Peer5
2. **Only one path** - No alternate routes to choose from
3. **Small scale** - Only 5 nodes, easy to configure manually
4. **Predictable** - Routes never change unexpectedly
5. **Simple** - No protocol overhead or complexity

**CCP would add complexity without benefit for this topology.**

## When You'd Want CCP

CCP becomes valuable when:

1. **Large networks** - 10+ connectors with multiple paths
2. **Mesh topologies** - Multiple routes to same destination
3. **Dynamic topology** - Connectors join/leave frequently
4. **Load balancing** - Distribute traffic across multiple paths
5. **Automatic failover** - Route around failed connectors

### Example: Mesh Network

```
        Connector A
       /     |     \
      /      |      \
  Conn B  Conn C  Conn D
      \      |      /
       \     |     /
        Connector E
```

**Without CCP:**

- Manually configure all possible paths
- Update all connectors when topology changes
- No automatic failover

**With CCP:**

- Connectors discover all paths automatically
- Failover happens automatically
- Load balancing across multiple paths

## CCP Implementation Status

### In M2M Project

**Status:** ❌ Not implemented

**Current approach:** Static YAML routing

**Future consideration:** Could implement CCP for larger deployments

### In Other ILP Implementations

- **Rafiki** (Interledger Foundation) - Static routing with API management
- **Connector.land** - Static configuration
- **ILP Kit** - Manual route configuration

**Consensus:** Static routing is standard for current ILP deployments

## Summary

**CCP (Connector-to-Connector Protocol):**

- Dynamic route advertisement protocol for ILP (proposed, not standardized)
- Similar to BGP but for payment routing
- Uses cost/liquidity metrics instead of AS path
- Not implemented in M2M (uses static routing instead)

**For your 5-peer linear chain:**

- Static routing is appropriate ✅
- CCP would add unnecessary complexity
- Consider CCP for larger, dynamic topologies

**Payment channel "handshake":**

- NOT CCP (CCP is for routing, not channels)
- NOT SPSP (SPSP is for end-user payments)
- IS Epic 17 BTP `payment-channel-claim` sub-protocol ✅
