# ILP Routing Configuration

## Are Routing Tables Hard-Coded?

**Yes, currently routing tables are statically configured** in YAML files.

### Current Approach: Static Configuration

Each peer has hardcoded routes in its YAML config:

```yaml
routes:
  - prefix: g.peer1
    nextHop: peer1
    priority: 0
  - prefix: g.peer2
    nextHop: peer2
    priority: 0
  - prefix: g.peer5
    nextHop: peer2 # Route via peer2
    priority: 0
```

**This means:**

- ✅ Simple and predictable
- ✅ No route discovery overhead
- ✅ Full control over routing
- ❌ Manual configuration required
- ❌ No automatic adaptation to network changes
- ❌ No load balancing or failover

### Alternative: Dynamic Routing

ILP supports dynamic routing via:

1. **CCP (Connector-to-Connector Protocol)** - RFC not yet implemented
   - Connectors broadcast routes to peers
   - Automatic route updates
   - Similar to BGP in traditional networking

2. **ILDCP (Interledger Dynamic Configuration Protocol)** - RFC-0031
   - Peer queries for its ILP address
   - Connector provides configuration
   - Dynamic address assignment

3. **Peer Discovery** - Not in core ILP RFCs
   - Service registry pattern
   - DNS-based discovery
   - Gossip protocols

### For Your Multi-Hop Network

**Currently:** Static YAML routing (5 peers, linear chain)

- Each peer knows where to forward based on destination prefix
- Routes manually configured for g.peer1 through g.peer5

**To enable dynamic routing:**

- Would need to implement CCP or similar protocol
- Connectors would exchange routing information
- Routes would update automatically

**For 5-peer linear topology:** Static routing is appropriate and sufficient.
