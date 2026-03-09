# Database Schema

**Decision: No Database Required for MVP**

The architecture uses **in-memory data structures only** with no persistence layer.

**Rationale:**

- Routing tables configured at startup from YAML files (ephemeral)
- Packet history not persisted (real-time observability only)
- Connector state resets on container restart (acceptable for dev/test tool)
- Simplifies architecture and reduces dependencies
- Aligns with educational/testing use case (no production data)

**Data Storage Strategy:**

- **Routing Tables:** In-memory Map/Array in each ConnectorNode
- **Peer Connections:** In-memory Map in BTPClientManager
- **Telemetry Events:** Emitted to structured logging, not stored
- **Logs:** Output to stdout, aggregated by Docker logging driver
- **Agent Wallet State:** SQLite database for payment channel tracking
- **Account Balances:** TigerBeetle distributed accounting system

**Current Databases:**

- **TigerBeetle:** High-performance distributed accounting for agent wallet balances
- **SQLite:** Agent wallet state, payment channel tracking, claim storage

**Post-MVP Considerations:**
Future versions might add:

- Redis for shared routing table state (multi-instance connectors)
- TimescaleDB for performance metrics storage
- Packet history database for forensics

**Note:** Dashboard visualization deferred - see DASHBOARD-DEFERRED.md in root
