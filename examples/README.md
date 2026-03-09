# ILP Connector Configuration Examples

This directory contains example YAML configuration files for deploying ILP connectors.

## Configuration Format

Each connector configuration file follows this YAML schema:

```yaml
# Node Identity
nodeId: string # Unique identifier for this connector
ilpAddress: string # ILP address prefix (e.g., g.mynode)
btpServerPort: number # Port for incoming BTP connections (1-65535)
logLevel: string # Optional: 'debug' | 'info' | 'warn' | 'error' (default: 'info')
healthCheckPort: number # Optional: HTTP health endpoint port (default: 8080)
explorerPort: number # Optional: Explorer UI port (default: 5173)

# Peer Connections
peers:
  - id: string # Peer identifier (used in routes)
    url: string # WebSocket URL (ws://host:port or wss://host:port)
    authToken: string # Shared secret for BTP authentication

# Routing Table
routes:
  - prefix: string # ILP address prefix (RFC-0015 format)
    nextHop: string # Peer ID from peers list, or own nodeId for local delivery
    priority: number # Optional: Route priority (default: 0, lower wins)

# Settlement Configuration
settlement:
  enableSettlement: boolean
  settlementThreshold: number
  connectorFeePercentage: number
  initialDepositMultiplier: number
  tigerBeetleClusterId: number
  tigerBeetleReplicas:
    - string # TigerBeetle replica address

settlementPreference: evm
```

## Example Files

### Production Single Node

- `production-single-node.yaml` - Production-hardened template for a single connector node

### Multi-Hop Topology (5 Peers)

A 5-node linear chain demonstrating multi-hop packet forwarding with EVM settlement.

**Topology**: Peer1 → Peer2 → Peer3 → Peer4 → Peer5

- `multihop-peer1.yaml` - Entry node
- `multihop-peer2.yaml` - Transit node 1
- `multihop-peer3.yaml` - Transit node 2 (middle)
- `multihop-peer4.yaml` - Transit node 3
- `multihop-peer5.yaml` - Exit node (destination)

### E2E Test Configs

- `e2e-connector-a.yaml` - Connector A for end-to-end tests
- `e2e-connector-b.yaml` - Connector B for end-to-end tests

## Usage

### Environment Variables

- `CONFIG_FILE`: Path to YAML configuration file (default: `./config.yaml`)
- `LOG_LEVEL`: Logging verbosity - `debug`, `info`, `warn`, `error` (default: `info`)

### Running a Connector

```bash
# Build the connector
npm run build --workspace=packages/connector

# Run with configuration
CONFIG_FILE=examples/production-single-node.yaml npm start --workspace=packages/connector
```

### Docker Compose Usage

```bash
# Start dev topology
docker compose -f docker-compose-dev.yml up -d

# View logs
docker compose -f docker-compose-dev.yml logs -f

# Stop network
docker compose -f docker-compose-dev.yml down
```

## Important Configuration Guidelines

1. **Unique Node IDs**: Each connector must have a unique `nodeId`
2. **Unique Ports**: Each connector must use a different `btpServerPort`
3. **Matching Peer IDs**: Route `nextHop` values must match peer `id` values
4. **Valid ILP Prefixes**: Route prefixes must follow RFC-0015 format
5. **WebSocket URLs**: Peer URLs must use `ws://` or `wss://` protocol
6. **Local Routes**: Include a route with `nextHop` matching the node's own `nodeId` for local packet delivery

## Additional Resources

- [Interledger RFC-0027: ILPv4](https://interledger.org/rfcs/0027-interledger-protocol-4/)
- [Interledger RFC-0023: Bilateral Transfer Protocol](https://interledger.org/rfcs/0023-bilateral-transfer-protocol/)
- [Interledger RFC-0015: ILP Addresses](https://interledger.org/rfcs/0015-ilp-addresses/)
- [Project Documentation](../docs/)
