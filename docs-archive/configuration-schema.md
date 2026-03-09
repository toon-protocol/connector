# Configuration Schema Documentation

This document provides comprehensive documentation for the ILP connector YAML configuration format. Use this guide to create custom network topologies and configure connector behavior.

## Table of Contents

- [Top-Level Configuration Fields](#top-level-configuration-fields)
- [Peer Configuration](#peer-configuration)
- [Routing Table Configuration](#routing-table-configuration)
- [Topology Examples](#topology-examples)
  - [Linear Topology (3 Nodes)](#linear-topology-3-nodes)
  - [Hub-and-Spoke Topology (4 Nodes)](#hub-and-spoke-topology-4-nodes)
  - [Full Mesh Topology (4 Nodes)](#full-mesh-topology-4-nodes)
- [Validation Rules](#validation-rules)
- [Common Pitfalls](#common-pitfalls)
- [BTP Bidirectional Authentication](#btp-bidirectional-authentication)

## Top-Level Configuration Fields

Complete connector configuration structure:

```yaml
# Required Fields
nodeId: string # Unique connector identifier
btpServerPort: number # Port for incoming BTP connections (1-65535)
peers: Peer[] # Outgoing BTP connections to peer connectors
routes: RoutingTableEntry[] # Initial routing table entries

# Optional Fields
healthCheckPort: number # HTTP health endpoint port (default: 8080)
logLevel: string # Logging verbosity (default: 'info')
dashboardTelemetryUrl: string # WebSocket URL for telemetry emission
```

### Field Descriptions

#### `nodeId: string` (required)

Unique identifier for this connector instance.

- **Type**: String
- **Required**: Yes
- **Constraints**: Non-empty, should be unique across the network
- **Examples**: `connector-a`, `hub`, `spoke-1`, `my-connector-node`

**Usage**:

```yaml
nodeId: connector-a
```

The `nodeId` is used for:

- Logging and telemetry identification
- Network topology visualization
- Peer identification in other connector's peer lists

---

#### `btpServerPort: number` (required)

Port number for the BTP (Bilateral Transfer Protocol) server to listen on. This is where the connector accepts incoming BTP connections from peers.

- **Type**: Number
- **Required**: Yes
- **Valid Range**: 1-65535
- **Common Values**: 3000, 3001, 3002, etc.

**Usage**:

```yaml
btpServerPort: 3000
```

**Port Allocation Guidelines**:

- Use sequential ports for multi-node deployments: 3000, 3001, 3002, ...
- Ensure no port conflicts with other services
- In Docker Compose, map container port to host port: `"3000:3000"`

---

#### `healthCheckPort: number` (optional)

Port number for the HTTP health check endpoint. Used by orchestration systems (Docker, Kubernetes) to monitor connector health.

- **Type**: Number
- **Required**: No
- **Default**: 8080
- **Valid Range**: 1-65535

**Usage**:

```yaml
healthCheckPort: 8080
```

The health endpoint provides:

- Current health status (`healthy`, `unhealthy`, `starting`)
- Uptime in seconds
- BTP peer connection statistics
- Node identification

**Endpoint**: `GET http://localhost:{healthCheckPort}/health`

---

#### `logLevel: string` (optional)

Logging verbosity level for structured JSON logs (Pino format).

- **Type**: String
- **Required**: No
- **Default**: `info`
- **Valid Values**: `debug`, `info`, `warn`, `error`

**Usage**:

```yaml
logLevel: info
```

**Log Levels**:

- **`debug`**: Detailed debugging information (packet contents, state transitions)
- **`info`**: General informational messages (connections established, routes updated)
- **`warn`**: Warning messages (connection retries, routing conflicts)
- **`error`**: Error messages only (connection failures, packet errors)

---

#### `peers: Peer[]` (required)

List of peer connectors to establish outgoing BTP connections with. Each peer represents another connector in the network.

- **Type**: Array of Peer objects
- **Required**: Yes (can be empty array)
- **Constraints**: Peer IDs must be unique within this list

**Usage**:

```yaml
peers:
  - id: connector-b
    url: ws://connector-b:3001
    authToken: secret-a-to-b

  - id: connector-c
    url: ws://connector-c:3002
    authToken: secret-a-to-c
```

**Empty Peers Array** (for hub nodes that only accept connections):

```yaml
peers: []
```

See [Peer Configuration](#peer-configuration) for detailed field descriptions.

---

#### `routes: RoutingTableEntry[]` (required)

Initial routing table entries defining how to forward packets based on destination address.

- **Type**: Array of RoutingTableEntry objects
- **Required**: Yes (can be empty array)
- **Constraints**: `nextHop` values must reference peer IDs from the `peers` list

**Usage**:

```yaml
routes:
  - prefix: g.alice
    nextHop: connector-b
    priority: 0

  - prefix: g.bob
    nextHop: connector-c
    priority: 10
```

See [Routing Table Configuration](#routing-table-configuration) for detailed field descriptions.

---

#### `dashboardTelemetryUrl: string` (optional)

WebSocket URL for sending telemetry events to the visualization dashboard.

- **Type**: String
- **Required**: No
- **Format**: `ws://hostname:port` or `wss://hostname:port`

**Usage**:

```yaml
dashboardTelemetryUrl: ws://dashboard:9000
```

When configured, the connector emits telemetry events for:

- Node status updates
- BTP connection state changes
- Packet flow events (prepare, fulfill, reject)
- Routing table updates

---

## Peer Configuration

Peer objects define outgoing BTP connections to other connectors.

### Peer Object Structure

```yaml
id: string # Peer identifier (required)
url: string # WebSocket URL for BTP connection (required)
authToken: string # Shared secret for authentication (required)
```

### Field Descriptions

#### `id: string` (required)

Unique identifier for this peer. Must match the peer's `nodeId` in its configuration.

- **Type**: String
- **Required**: Yes
- **Constraints**: Must be unique within the `peers` array
- **Usage**: Referenced in `route.nextHop` fields

**Example**:

```yaml
peers:
  - id: connector-b # Must match nodeId in connector-b's config
    url: ws://connector-b:3001
    authToken: secret-a-to-b
```

---

#### `url: string` (required)

WebSocket URL for connecting to the peer's BTP server.

- **Type**: String
- **Required**: Yes
- **Format**: `ws://hostname:port` or `wss://hostname:port`
- **Port**: Must match peer's `btpServerPort`

**Examples**:

```yaml
# Docker Compose service name resolution
url: ws://connector-b:3001

# IP address
url: ws://192.168.1.10:3000

# Hostname
url: ws://peer-connector.example.com:3000

# Secure WebSocket (wss)
url: wss://secure-connector.example.com:3001
```

**Docker Compose Note**: Use container service names as hostnames (Docker DNS resolution).

---

#### `authToken: string` (required)

Shared secret for BTP authentication. Used to authenticate this connector to the peer.

- **Type**: String
- **Required**: Yes
- **Constraints**: Should be a strong, randomly generated token
- **Recommendation**: Use unique tokens for each peer connection

**Examples**:

```yaml
# Simple format (suitable for testing/MVP)
authToken: secret-a-to-b

# Strong token (recommended for production)
authToken: 8f3e2d1c-4b5a-6c7d-8e9f-0a1b2c3d4e5f

# Base64-encoded random bytes (high security)
authToken: k8sJd9f2KLs0d8fj2lskd9f2klsd0f9j2lksd0f9
```

**Security Note**: Shared secrets should match on both sides of the connection (bidirectional authentication).

---

## Routing Table Configuration

Routing table entries define how to forward ILP packets based on destination address prefixes.

### RoutingTableEntry Object Structure

```yaml
prefix: string # ILP address prefix pattern (required)
nextHop: string # Peer ID to forward packets to (required)
priority: number # Route priority for tie-breaking (optional, default: 0)
```

### Field Descriptions

#### `prefix: string` (required)

ILP address prefix pattern for route matching (RFC-0015 format).

- **Type**: String
- **Required**: Yes
- **Format**: Lowercase alphanumeric, dots, underscores, tildes, hyphens
- **Pattern**: `^[a-z0-9][a-z0-9._~-]*$`

**Examples**:

```yaml
# Match all addresses starting with "g.alice"
prefix: g.alice

# Match all addresses starting with "g.bob.usd"
prefix: g.bob.usd

# Catch-all route (matches all addresses starting with "g")
prefix: g
```

**Longest Prefix Match**: The connector uses longest prefix matching for route selection. More specific routes take precedence over catch-all routes.

---

#### `nextHop: string` (required)

Peer ID to forward matching packets to. Must reference an existing peer ID from the `peers` array.

- **Type**: String
- **Required**: Yes
- **Constraints**: Must match a `peer.id` in the `peers` list

**Example**:

```yaml
peers:
  - id: connector-b # Define peer
    url: ws://connector-b:3001
    authToken: secret-a-to-b

routes:
  - prefix: g.alice
    nextHop: connector-b # Reference peer ID
```

**Multi-Hop Routing**: Intermediate connectors forward packets through the network using their own routing tables, enabling multi-hop paths.

---

#### `priority: number` (optional)

Route priority for tie-breaking when multiple routes match the same prefix.

- **Type**: Number
- **Required**: No
- **Default**: 0
- **Behavior**: Higher priority routes are preferred

**Usage**:

```yaml
routes:
  # Primary route (high priority)
  - prefix: g.alice
    nextHop: connector-b
    priority: 10

  # Backup route (low priority)
  - prefix: g.alice
    nextHop: connector-c
    priority: 5
```

---

## Topology Examples

### Linear Topology (3 Nodes)

A linear chain topology where packets flow through intermediate nodes:

```
Connector A → Connector B → Connector C
```

**Connector A Configuration** (`examples/linear-3-nodes-a.yaml`):

```yaml
# Connector A: Entry point of linear chain
# Topology: A → B → C
#
# This connector only connects to B. To reach C, packets
# must be forwarded through B (multi-hop routing).

# Node Identity
nodeId: connector-a
btpServerPort: 3000
logLevel: info
healthCheckPort: 8080

# Peer Connections
# A connects to B only (not directly to C)
peers:
  - id: connector-b
    url: ws://connector-b:3001
    authToken: secret-a-to-b

# Routing Table
# A routes to B (direct) and C (via B - multi-hop)
routes:
  # Direct route to B
  - prefix: g.connectorb
    nextHop: connector-b
    priority: 0

  # Multi-hop route to C (via B)
  - prefix: g.connectorc
    nextHop: connector-b # B will forward to C
    priority: 0
```

**Connector B Configuration** (`examples/linear-3-nodes-b.yaml`):

```yaml
# Connector B: Transit node in linear chain
# Topology: A ← B → C
#
# This connector connects to both A and C, acting as a relay
# point for traffic between them.

# Node Identity
nodeId: connector-b
btpServerPort: 3001
logLevel: info
healthCheckPort: 8080

# Peer Connections
# B connects to both A and C
peers:
  - id: connector-a
    url: ws://connector-a:3000
    authToken: secret-a-to-b

  - id: connector-c
    url: ws://connector-c:3002
    authToken: secret-b-to-c

# Routing Table
# B routes to A (direct) and C (direct)
routes:
  # Route to A
  - prefix: g.connectora
    nextHop: connector-a
    priority: 0

  # Route to C
  - prefix: g.connectorc
    nextHop: connector-c
    priority: 0
```

**Connector C Configuration** (`examples/linear-3-nodes-c.yaml`):

```yaml
# Connector C: Exit point of linear chain
# Topology: A → B ← C
#
# This connector only connects to B. To reach A, packets
# must be forwarded through B (multi-hop routing).

# Node Identity
nodeId: connector-c
btpServerPort: 3002
logLevel: info
healthCheckPort: 8080

# Peer Connections
# C connects to B only (not directly to A)
peers:
  - id: connector-b
    url: ws://connector-b:3001
    authToken: secret-b-to-c

# Routing Table
# C routes to B (direct) and A (via B - multi-hop)
routes:
  # Direct route to B
  - prefix: g.connectorb
    nextHop: connector-b
    priority: 0

  # Multi-hop route to A (via B)
  - prefix: g.connectora
    nextHop: connector-b # B will forward to A
    priority: 0
```

**Key Characteristics**:

- **Multi-Hop Routing**: A→C requires forwarding through B
- **Single Point of Failure**: B is critical (failure disconnects A and C)
- **Simple Configuration**: Each node has 1-2 peer connections

---

### Hub-and-Spoke Topology (4 Nodes)

A centralized topology where a hub connector manages all inter-spoke communication:

```
         Spoke-1
             ↓
       ← Hub (center) →
      ↙           ↘
  Spoke-2       Spoke-3
```

**Hub Configuration** (`examples/hub-spoke-hub.yaml`):

```yaml
# Hub-and-Spoke Topology: Hub Connector
# Topology: Hub ← Spoke1, Spoke2, Spoke3
#
# This is the hub connector in a hub-and-spoke topology.
# The hub acts as a central relay point for all spoke connectors.
# All spokes connect to the hub, and all inter-spoke traffic flows through the hub.

# Node Identity
nodeId: connector-hub
btpServerPort: 3000
logLevel: info
healthCheckPort: 8080

# Peer Connections
# Hub accepts incoming connections from spokes, does not initiate connections
# peers array is EMPTY - hub is passive, spokes connect to it
peers: []

# Routing Table
# Routes for each spoke connector
# NOTE: These routes only work once spokes establish connections
# The hub references spoke peer IDs even though it doesn't initiate connections
routes:
  - prefix: g.spoke1
    nextHop: spoke-1 # Spoke-1 will connect and identify as "spoke-1"
    priority: 0

  - prefix: g.spoke2
    nextHop: spoke-2
    priority: 0

  - prefix: g.spoke3
    nextHop: spoke-3
    priority: 0
```

**Spoke-1 Configuration** (`examples/hub-spoke-spoke1.yaml`):

```yaml
# Hub-and-Spoke Topology: Spoke 1
# Topology: Hub ← Spoke1 (this node)

# Node Identity
nodeId: spoke-1
btpServerPort: 3001
logLevel: info
healthCheckPort: 8080

# Peer Connections
# Spoke connects only to the hub
peers:
  - id: connector-hub
    url: ws://connector-hub:3000
    authToken: secret-spoke1-to-hub

# Routing Table
# All traffic routed through the hub
routes:
  # Routes to other spokes (via hub)
  - prefix: g.spoke2
    nextHop: connector-hub # Hub will forward to spoke-2
    priority: 0

  - prefix: g.spoke3
    nextHop: connector-hub # Hub will forward to spoke-3
    priority: 0

  # Catch-all route - send everything else to hub
  - prefix: g
    nextHop: connector-hub
    priority: 0
```

**Key Characteristics**:

- **Centralized Control**: Hub is single point of control
- **Empty Peers Array**: Hub has `peers: []` (accepts connections only)
- **Multi-Hop Inter-Spoke Traffic**: Spoke1→Hub→Spoke2 (two hops)
- **Scalability**: Easy to add new spokes without reconfiguring existing nodes

---

### Full Mesh Topology (4 Nodes)

A fully connected topology where every connector has direct connections to all others:

```
    A ←→ B
    ↕ ✕ ↕
    D ←→ C

All connections are bidirectional BTP peerings.
```

**Connector A Configuration** (`examples/mesh-4-nodes-a.yaml`):

```yaml
# Full Mesh Topology: Connector A
# Topology: A has direct connections to B, C, and D

# Node Identity
nodeId: connector-a
btpServerPort: 3000
logLevel: info
healthCheckPort: 8080

# Peer Connections
# A connects to all other nodes (B, C, D)
peers:
  - id: connector-b
    url: ws://connector-b:3001
    authToken: secret-a-to-b

  - id: connector-c
    url: ws://connector-c:3002
    authToken: secret-a-to-c

  - id: connector-d
    url: ws://connector-d:3003
    authToken: secret-a-to-d

# Routing Table
# A has direct routes to all nodes (1-hop)
routes:
  - prefix: g.connectorb
    nextHop: connector-b
    priority: 0

  - prefix: g.connectorc
    nextHop: connector-c
    priority: 0

  - prefix: g.connectord
    nextHop: connector-d
    priority: 0
```

**Key Characteristics**:

- **Direct Routing**: Any node can reach any other in one hop
- **Redundant Paths**: Multiple routes available for fault tolerance
- **O(n²) Connections**: 4 nodes = 6 bidirectional connections (scales poorly)
- **High Resilience**: Network remains functional even if some connections fail

---

## Validation Rules

The configuration loader and topology validator perform the following checks:

### Single-Config Validation (ConfigLoader)

These checks are performed when loading individual configuration files:

1. **Required Field Presence**:
   - `nodeId` must be present and non-empty string
   - `btpServerPort` must be present and valid number
   - `peers` must be present and array type
   - `routes` must be present and array type

2. **Port Range Validation**:
   - `btpServerPort` must be 1-65535
   - `healthCheckPort` (if present) must be 1-65535

3. **Log Level Validation**:
   - `logLevel` (if present) must be: `debug`, `info`, `warn`, or `error`

4. **Peer Validation**:
   - Each peer must have `id`, `url`, and `authToken` fields
   - Peer IDs must be unique within `peers` array
   - Peer URLs must match WebSocket format: `ws://host:port` or `wss://host:port`

5. **Route Validation**:
   - Each route must have `prefix` and `nextHop` fields
   - `prefix` must match ILP address format (RFC-0015): `^[a-z0-9][a-z0-9._~-]*$`
   - `nextHop` must reference an existing `peer.id` in the `peers` array
   - `priority` (if present) must be a number

### Multi-Config Topology Validation (TopologyValidator)

These checks are performed across all connector configurations in a topology:

1. **Disconnected Node Detection** (ERROR):
   - All nodes must be reachable via BTP peer connections
   - A node is disconnected if it has no path to any other node
   - Uses graph traversal (DFS) to detect isolated nodes

2. **Invalid Peer Reference Detection** (ERROR):
   - All `peer.id` values must match a `nodeId` in another config
   - Peer references to non-existent nodes are errors

3. **Unreachable Destination Detection** (WARNING):
   - Routes with `nextHop` pointing to non-connected peers generate warnings
   - Multi-hop reachability analysis identifies unreachable destinations
   - Does not prevent deployment but alerts operator

4. **Circular Route Dependency Detection** (ERROR):
   - Detects routing loops: A→B→C→A
   - Uses cycle detection algorithm (DFS with recursion stack)
   - Circular dependencies indicate misconfigured routing

### Running Validation

**Single Config Validation**:

```bash
# Automatic validation when loading config
node dist/index.js --config examples/hub-spoke-hub.yaml
```

**Topology Validation**:

```bash
# Use validation script (Story 4.3 Task 9)
node tools/validate-topology.js --config-dir examples/ \
  --files hub-spoke-hub.yaml,hub-spoke-spoke1.yaml,hub-spoke-spoke2.yaml,hub-spoke-spoke3.yaml
```

---

## Common Pitfalls

### 1. Peer ID Mismatch

**Problem**: Peer ID doesn't match target connector's `nodeId`.

**Example** (WRONG):

```yaml
# Connector A config
peers:
  - id: my-peer-b # ❌ Wrong ID
    url: ws://connector-b:3001
    authToken: secret

# Connector B config
nodeId: connector-b # ❌ Mismatch!
```

**Solution**: Ensure `peer.id` matches target's `nodeId` exactly:

```yaml
# Connector A config
peers:
  - id: connector-b # ✅ Matches
    url: ws://connector-b:3001
    authToken: secret

# Connector B config
nodeId: connector-b # ✅ Match!
```

---

### 2. NextHop Referencing Non-Existent Peer

**Problem**: Route `nextHop` references a peer that's not in the `peers` array.

**Example** (WRONG):

```yaml
peers:
  - id: connector-b
    url: ws://connector-b:3001
    authToken: secret

routes:
  - prefix: g.alice
    nextHop: connector-c # ❌ connector-c not in peers!
```

**Solution**: Add the peer or fix the `nextHop` reference:

```yaml
peers:
  - id: connector-b
    url: ws://connector-b:3001
    authToken: secret

  - id: connector-c # ✅ Add peer
    url: ws://connector-c:3002
    authToken: secret-c

routes:
  - prefix: g.alice
    nextHop: connector-c # ✅ Now valid
```

---

### 3. Disconnected Nodes

**Problem**: Node has no BTP connections to the rest of the network.

**Example** (WRONG):

```yaml
# Connector A: connects to B
peers:
  - id: connector-b
    url: ws://connector-b:3001
    authToken: secret

# Connector B: connects to A
peers:
  - id: connector-a
    url: ws://connector-a:3000
    authToken: secret

# Connector C: NO PEERS! ❌ Disconnected from A-B
peers: []
```

**Topology Validator Error**:

```
Disconnected nodes detected: connector-c.
All nodes must be connected via BTP peer relationships.
```

**Solution**: Connect C to the network:

```yaml
# Connector C: connect to B
peers:
  - id: connector-b
    url: ws://connector-b:3001
    authToken: secret
```

---

### 4. Unreachable Routes

**Problem**: Route with `nextHop` that doesn't have a BTP connection.

**Example** (WARNING):

```yaml
# Hub configuration
peers: [] # Hub accepts connections but doesn't initiate

routes:
  - prefix: g.spoke1
    nextHop: spoke-1 # ⚠️ spoke-1 not connected yet
```

**Topology Validator Warning**:

```
Node connector-hub: Route to g.spoke1 unreachable (nextHop spoke-1 not connected)
```

**Explanation**: This is acceptable for hub-and-spoke topologies where spokes connect TO the hub. The route becomes valid once spoke-1 establishes its connection. This generates a warning (not an error) because it may be intentional.

---

### 5. Shared Secret Mismatch

**Problem**: `authToken` values don't match on both sides of a connection.

**Example** (WRONG):

```yaml
# Connector A config
peers:
  - id: connector-b
    url: ws://connector-b:3001
    authToken: secret-a-to-b  # ❌ Token 1

# Connector B config
peers:
  - id: connector-a
    url: ws://connector-a:3000
    authToken: secret-b-to-a  # ❌ Token 2 (different!)
```

**Result**: BTP authentication failures, connection rejected.

**Solution**: Use the same token on both sides:

```yaml
# Connector A config
peers:
  - id: connector-b
    url: ws://connector-b:3001
    authToken: secret-a-to-b  # ✅ Same token

# Connector B config
peers:
  - id: connector-a
    url: ws://connector-a:3000
    authToken: secret-a-to-b  # ✅ Same token
```

---

### 6. Invalid ILP Address Prefix Format

**Problem**: Route prefix doesn't match RFC-0015 format.

**Example** (WRONG):

```yaml
routes:
  # Invalid: uppercase letters
  - prefix: G.Alice # ❌ Uppercase not allowed
    nextHop: connector-b

  # Invalid: spaces
  - prefix: g.bob usd # ❌ Spaces not allowed
    nextHop: connector-b

  # Invalid: special characters
  - prefix: g.alice@usd # ❌ @ not allowed
    nextHop: connector-b
```

**Solution**: Use valid ILP address format (lowercase alphanumeric, dots, underscores, tildes, hyphens):

```yaml
routes:
  - prefix: g.alice # ✅ Valid
    nextHop: connector-b

  - prefix: g.bob.usd # ✅ Valid (dot separator)
    nextHop: connector-b

  - prefix: g.alice_usd # ✅ Valid (underscore)
    nextHop: connector-b
```

---

## BTP Bidirectional Authentication

The BTP server uses a **two-part authentication mechanism** to support bidirectional connections between connectors.

### Overview

BTP authentication involves **two separate configurations**:

1. **Outgoing Connections (Client-Side)**: Configured in YAML `peers` array
2. **Incoming Connections (Server-Side)**: Configured via Docker Compose environment variables

This design enables bidirectional connections where connector-a connects to connector-b AND connector-b connects to connector-a simultaneously.

---

### Client-Side Authentication (Outgoing)

**Configured in**: YAML `peers` array
**Purpose**: Authenticate THIS connector TO its peer

When connector-a wants to connect to connector-b, it uses the `authToken` from its YAML configuration:

```yaml
# Connector A configuration (connector-a.yaml)
peers:
  - id: connector-b
    url: ws://connector-b:3001
    authToken: secret-a-to-b # Token sent when A connects to B
```

Connector-a sends `authToken: secret-a-to-b` when connecting to connector-b.

---

### Server-Side Authentication (Incoming)

**Configured in**: Docker Compose environment variables
**Purpose**: Authenticate INCOMING peers connecting TO this connector

When connector-b wants to accept connections FROM connector-a, it needs an environment variable:

```yaml
# docker-compose.yml
services:
  connector-b:
    environment:
      BTP_PEER_CONNECTOR_A_SECRET: secret-a-to-b
```

Connector-b verifies that the incoming peer "connector-a" provides the matching token.

---

### Environment Variable Naming Convention

**Pattern**: `BTP_PEER_<PEER_ID>_SECRET`

Where `<PEER_ID>` is the peer's `nodeId` transformed:

- Converted to UPPERCASE
- Hyphens (`-`) replaced with underscores (`_`)

**Examples**:

| Peer Node ID         | Environment Variable                 |
| -------------------- | ------------------------------------ |
| `connector-a`        | `BTP_PEER_CONNECTOR_A_SECRET`        |
| `spoke-1`            | `BTP_PEER_SPOKE_1_SECRET`            |
| `hub-2`              | `BTP_PEER_HUB_2_SECRET`              |
| `send-packet-client` | `BTP_PEER_SEND_PACKET_CLIENT_SECRET` |

**Code Reference**: `packages/connector/src/btp/btp-server.ts:486`

```typescript
const envVarKey = `BTP_PEER_${peerId.toUpperCase().replace(/-/g, '_')}_SECRET`;
const expectedSecret = process.env[envVarKey];
```

---

### Bidirectional Authentication Setup

For a **bidirectional connection** between connector-a and connector-b, you need BOTH:

**Connector-a YAML** (`examples/connector-a.yaml`):

```yaml
peers:
  - id: connector-b
    url: ws://connector-b:3001
    authToken: secret-a-to-b # A's token when connecting to B
```

**Connector-a Docker Compose Environment**:

```yaml
# docker-compose.yml
services:
  connector-a:
    environment:
      BTP_PEER_CONNECTOR_B_SECRET: secret-b-to-a # B's token when connecting to A
```

**Connector-b YAML** (`examples/connector-b.yaml`):

```yaml
peers:
  - id: connector-a
    url: ws://connector-a:3000
    authToken: secret-b-to-a # B's token when connecting to A
```

**Connector-b Docker Compose Environment**:

```yaml
# docker-compose.yml
services:
  connector-b:
    environment:
      BTP_PEER_CONNECTOR_A_SECRET: secret-a-to-b # A's token when connecting to B
```

---

### Symmetric vs. Asymmetric Tokens

**Symmetric Tokens (Recommended)**:
Use the **same token value** for both directions. This simplifies configuration.

```yaml
# Connector A
peers:
  - id: connector-b
    authToken: shared-token-a-b

# Connector B
peers:
  - id: connector-a
    authToken: shared-token-a-b

# Docker Compose
connector-a:
  environment:
    BTP_PEER_CONNECTOR_B_SECRET: shared-token-a-b

connector-b:
  environment:
    BTP_PEER_CONNECTOR_A_SECRET: shared-token-a-b
```

**Asymmetric Tokens (Advanced)**:
Use different tokens for each direction. More complex but allows per-direction security policies.

---

### Common Authentication Errors

#### Error: "Authentication failed: peer not configured"

**Cause**: Missing environment variable for incoming peer.

**Example**:

```
{"level":40,"msg":"BTP authentication failed: peer not configured","peerId":"connector-b"}
```

**Solution**: Add environment variable to Docker Compose:

```yaml
connector-a:
  environment:
    BTP_PEER_CONNECTOR_B_SECRET: secret-b-to-a
```

---

#### Error: "Authentication failed: invalid secret"

**Cause**: Token mismatch between YAML `authToken` and environment variable.

**Example**:

```
{"level":40,"msg":"BTP authentication failed: invalid secret","peerId":"connector-b"}
```

**Solution**: Ensure tokens match:

```yaml
# Connector B YAML
peers:
  - id: connector-a
    authToken: secret-b-to-a # This value...

# Connector A Docker Compose
connector-a:
  environment:
    BTP_PEER_CONNECTOR_B_SECRET: secret-b-to-a # ...must match this
```

---

### Topology-Specific Authentication Patterns

#### Linear Topology (Unidirectional)

Linear topologies typically use **unidirectional connections** where only certain nodes initiate:

```yaml
# docker-compose.yml
connector-a:
  environment:
    # No environment variables needed (only initiates connections)

connector-b:
  environment:
    BTP_PEER_CONNECTOR_A_SECRET: secret-a-to-b
    BTP_PEER_CONNECTOR_C_SECRET: secret-b-to-c

connector-c:
  environment:
    # No environment variables needed (only initiates connections)
```

#### Mesh Topology (Bidirectional)

Mesh topologies require **bidirectional authentication** on ALL nodes:

```yaml
# docker-compose.yml
connector-a:
  environment:
    BTP_PEER_CONNECTOR_B_SECRET: secret-a-to-b
    BTP_PEER_CONNECTOR_C_SECRET: secret-a-to-c
    BTP_PEER_CONNECTOR_D_SECRET: secret-a-to-d

connector-b:
  environment:
    BTP_PEER_CONNECTOR_A_SECRET: secret-a-to-b
    BTP_PEER_CONNECTOR_C_SECRET: secret-b-to-c
    BTP_PEER_CONNECTOR_D_SECRET: secret-b-to-d

# ... and so on for connector-c and connector-d
```

#### Hub-and-Spoke Topology (Hub-Only Server)

Hub-and-spoke topologies have the **hub as server only**:

```yaml
# docker-compose.yml
connector-hub:
  environment:
    # Hub accepts connections from all spokes
    BTP_PEER_SPOKE_1_SECRET: secret-spoke1-to-hub
    BTP_PEER_SPOKE_2_SECRET: secret-spoke2-to-hub
    BTP_PEER_SPOKE_3_SECRET: secret-spoke3-to-hub

spoke-1:
  environment:
    # Spokes don't accept connections (no environment variables needed)

spoke-2:
  environment:
    # Spokes don't accept connections (no environment variables needed)

spoke-3:
  environment:
    # Spokes don't accept connections (no environment variables needed)
```

---

## Next Steps

- **Deploy Custom Topology**: Use Docker Compose with custom configs
- **Monitor Network**: Access dashboard at `http://localhost:8080`
- **Validate Configuration**: Run `tools/validate-topology.js` before deployment
- **Troubleshoot Issues**: Check health endpoints and connector logs

For more information, see:

- [README.md](../README.md) - Quick start and Docker Compose usage
- [Architecture Documentation](architecture/) - System design and components
- [Example Configurations](../examples/) - Pre-configured topology files
