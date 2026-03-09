# Multi-Hop ILP Network Deployment Guide

This guide explains how to deploy and test a 5-peer ILP network with multi-hop packet routing.

## Overview

The multi-hop deployment creates a linear chain of 5 production ILP connectors:

```
┌─────────┐      ┌─────────┐      ┌─────────┐      ┌─────────┐      ┌─────────┐
│  Peer1  │─────▶│  Peer2  │─────▶│  Peer3  │─────▶│  Peer4  │─────▶│  Peer5  │
│ :3000   │      │ :3001   │      │ :3002   │      │ :3003   │      │ :3004   │
└─────────┘      └─────────┘      └─────────┘      └─────────┘      └─────────┘
g.peer1          g.peer2          g.peer3          g.peer4          g.peer5
```

**Key Features:**

- Each peer has a unique ILP address (`g.peer1` through `g.peer5`)
- Peers are funded from the base treasury wallet using EVM transactions
- Payment channels are established between consecutive peers for settlement
- Packets can traverse all 5 hops from Peer1 to Peer5
- **TigerBeetle**: High-performance accounting database tracks peer balances in real-time

## Prerequisites

Before deploying the multi-hop network, ensure you have:

1. **Docker and Docker Compose** installed

   ```bash
   docker --version  # Should be 20.10+
   docker-compose --version  # Should be 2.x
   ```

2. **Built connector image**

   ```bash
   docker build -t agent-runtime .
   ```

3. **Environment variables configured**

   Copy `.env.example` to `.env` and configure:

   ```bash
   cp .env.example .env
   ```

   Required environment variables:

   ```env
   # Treasury wallet private keys
   TREASURY_EVM_PRIVATE_KEY=0x...

   # Base L2 RPC URL (local Anvil or Base testnet)
   BASE_L2_RPC_URL=http://localhost:8545

   # Optional: Pre-generated peer addresses
   PEER1_EVM_ADDRESS=0x...
   PEER2_EVM_ADDRESS=0x...
   PEER3_EVM_ADDRESS=0x...
   PEER4_EVM_ADDRESS=0x...
   PEER5_EVM_ADDRESS=0x...
   ```

   > **Note:** If peer addresses are not provided, the funding script will generate new wallets.

4. **Local blockchain node** (optional but recommended for testing)

   Start Anvil (included in the Docker Compose):

   ```bash
   docker-compose -f docker-compose-5-peer-multihop.yml up -d anvil
   ```

   Or use a public testnet (e.g., Base Sepolia).

## Quick Start

### Option 1: Automated Deployment (Recommended)

Use the deployment script to automatically set up the entire network:

```bash
./scripts/deploy-5-peer-multihop.sh
```

This script will:

1. ✓ Check prerequisites (Docker, images, .env)
2. ✓ Start the 5-peer network
3. ✓ Wait for all peers to become healthy
4. ✓ Fund peers from the treasury wallet
5. ✓ Send a test packet through all 5 hops
6. ✓ Verify multi-hop routing behavior

### Option 2: Manual Deployment

If you prefer manual control:

#### Step 1: Start the network

```bash
docker-compose -f docker-compose-5-peer-multihop.yml up -d
```

#### Step 2: Wait for peers to become healthy

```bash
# Check all peers are running
docker-compose -f docker-compose-5-peer-multihop.yml ps

# Check health endpoints
curl http://localhost:9080/health  # Peer1
curl http://localhost:9081/health  # Peer2
curl http://localhost:9082/health  # Peer3
curl http://localhost:9083/health  # Peer4
curl http://localhost:9084/health  # Peer5
```

#### Step 3: Fund peers

```bash
cd tools/fund-peers
npm install
npm run build

npm run fund -- --peers peer1,peer2,peer3,peer4,peer5 \
  --eth-amount 0.1 \
  --token-amount 1000
```

#### Step 4: Send test packet

```bash
cd tools/send-packet
npm install
npm run build

npm run send -- \
  --connector-url ws://localhost:3000 \
  --destination g.peer5.dest \
  --amount 1000000 \
  --auth-token test-token
```

## Network Topology

### ILP Address Hierarchy

Each peer has a unique ILP address following the global address scheme:

```
g.peer1          Entry node (receives external packets)
g.peer2          Transit node 1
g.peer3          Transit node 2 (middle)
g.peer4          Transit node 3
g.peer5          Exit node (destination)
```

### Routing Tables

#### Peer1 (Entry)

```yaml
routes:
  - prefix: g.peer1 # Local delivery
    nextHop: peer1
  - prefix: g.peer2 # Route to peer2
    nextHop: peer2
  - prefix: g.peer3 # Route via peer2
    nextHop: peer2
  - prefix: g.peer4 # Route via peer2
    nextHop: peer2
  - prefix: g.peer5 # Route via peer2
    nextHop: peer2
```

#### Peer3 (Middle)

```yaml
routes:
  - prefix: g.peer1 # Route upstream via peer2
    nextHop: peer2
  - prefix: g.peer2 # Route upstream
    nextHop: peer2
  - prefix: g.peer3 # Local delivery
    nextHop: peer3
  - prefix: g.peer4 # Route downstream
    nextHop: peer4
  - prefix: g.peer5 # Route downstream via peer4
    nextHop: peer4
```

#### Peer5 (Exit)

```yaml
routes:
  - prefix: g.peer1 # All upstream via peer4
    nextHop: peer4
  - prefix: g.peer2 # All upstream via peer4
    nextHop: peer4
  - prefix: g.peer3 # All upstream via peer4
    nextHop: peer4
  - prefix: g.peer4 # Upstream
    nextHop: peer4
  - prefix: g.peer5 # Local delivery
    nextHop: peer5
```

### BTP Connections

BTP (Bilateral Transfer Protocol) connections are established as follows:

```
Peer1 (server) ←── Peer2 (client)
Peer2 (server) ←── Peer3 (client)
Peer3 (server) ←── Peer4 (client)
Peer4 (server) ←── Peer5 (client)
```

- Each peer runs a BTP server on its designated port
- Downstream peers initiate BTP client connections to upstream peers
- Authentication uses shared secrets defined in environment variables

## Multi-Hop Packet Flow

### PREPARE Packet (Forward Path)

When a packet is sent to destination `g.peer5.dest`:

1. **Peer1** (Entry)
   - Receives ILP PREPARE packet from external client
   - Performs longest-prefix matching: `g.peer5` → `nextHop: peer2`
   - Decrements expiry time (1 second)
   - Calculates connector fee (optional)
   - Forwards packet to Peer2 via BTP

2. **Peer2** (Transit 1)
   - Receives PREPARE from Peer1 via BTP
   - Routing lookup: `g.peer5` → `nextHop: peer3`
   - Decrements expiry time
   - Forwards packet to Peer3 via BTP

3. **Peer3** (Transit 2 - Middle)
   - Receives PREPARE from Peer2 via BTP
   - Routing lookup: `g.peer5` → `nextHop: peer4`
   - Decrements expiry time
   - Forwards packet to Peer4 via BTP

4. **Peer4** (Transit 3)
   - Receives PREPARE from Peer3 via BTP
   - Routing lookup: `g.peer5` → `nextHop: peer5`
   - Decrements expiry time
   - Forwards packet to Peer5 via BTP

5. **Peer5** (Destination)
   - Receives PREPARE from Peer4 via BTP
   - Routing lookup: `g.peer5.dest` → `nextHop: peer5` (local delivery)
   - Validates packet (expiry, condition, amount)
   - Generates FULFILL response with fulfillment preimage
   - Returns FULFILL to Peer4

### FULFILL Packet (Return Path)

The FULFILL packet propagates back through the chain:

```
Peer5 → Peer4 → Peer3 → Peer2 → Peer1 → Client
```

Each hop:

- Verifies fulfillment matches execution condition
- Records settlement (if enabled)
- Forwards FULFILL upstream

## Verification

### Checking Logs

View logs for all peers:

```bash
docker-compose -f docker-compose-5-peer-multihop.yml logs -f
```

View logs for a specific peer:

```bash
docker-compose -f docker-compose-5-peer-multihop.yml logs -f peer3
```

### Expected Log Events

For a successful multi-hop packet:

**Peer1 logs:**

```json
{"level":"info","msg":"Packet received","destination":"g.peer5.dest"}
{"level":"info","msg":"Routing decision","nextHop":"peer2"}
{"level":"info","msg":"Forwarding packet","peer":"peer2"}
{"level":"info","msg":"Received FULFILL","fulfillment":"..."}
```

**Peer3 logs (middle):**

```json
{"level":"info","msg":"Packet received from peer","peer":"peer2"}
{"level":"info","msg":"Routing decision","nextHop":"peer4"}
{"level":"info","msg":"Forwarding packet","peer":"peer4"}
{"level":"info","msg":"Received FULFILL from peer","peer":"peer4"}
```

**Peer5 logs:**

```json
{"level":"info","msg":"Packet received from peer","peer":"peer4"}
{"level":"info","msg":"Local delivery","destination":"g.peer5.dest"}
{"level":"info","msg":"Packet fulfilled","fulfillment":"..."}
```

### Verifying Multi-Hop Behavior

The deployment script automatically verifies multi-hop behavior by checking:

1. ✓ Each transit peer (1-4) received and forwarded PREPARE packets
2. ✓ Destination peer (5) delivered packet locally
3. ✓ FULFILL response returned through all hops
4. ✓ No packet rejections or timeouts

## Treasury Wallet Funding

### How It Works

Peers are funded from the base treasury wallet using standard EVM transactions:

1. **Treasury Wallet** (loaded from `TREASURY_EVM_PRIVATE_KEY`)
   - Holds ETH for gas fees
   - Holds ERC20 tokens for payment channel deposits

2. **Peer Wallets** (generated or loaded from environment)
   - Each peer has a unique Ethereum address
   - Receives ETH for transaction gas
   - Receives ERC20 tokens for settlement

3. **Funding Process**
   ```typescript
   // For each peer:
   const tx = await treasuryWallet.sendTransaction({
     to: peerAddress,
     value: ethers.parseEther('0.1'), // 0.1 ETH
   });
   ```

### Payment Channels

Payment channels are established between consecutive peers for off-chain settlement:

```
Peer1 ↔ Peer2 ↔ Peer3 ↔ Peer4 ↔ Peer5
  │       │       │       │       │
  └───────┴───────┴───────┴───────┘
     Payment Channels (ERC20)
```

**Channel Setup:**

- Initial deposit: `1000 tokens × 10` (multiplier)
- Settlement timeout: 24 hours
- Automatic refunding when balance drops below threshold

## Claims and Settlement (Epic 17)

### BTP Off-Chain Claim Exchange Protocol

The network uses **BTP (Bilateral Transfer Protocol) claim exchange** to send cryptographic settlement proofs between peers without requiring on-chain transactions for every settlement. This is implemented via Epic 17 components:

**Key Components:**

- **PerPacketClaimService** - Generates signed claims per outgoing PREPARE packet, attached via BTP protocolData (supersedes ClaimSender)
- **ClaimReceiver** - Receives and verifies claims from peers (supports dynamic on-chain verification for unknown channels)
- **ClaimRedemptionService** - Automatically redeems profitable claims on-chain
- **BTP `payment-channel-claim` sub-protocol** - Transports claims as protocolData on ILP PREPARE packets

**Supported Blockchains:**

- EVM/Base L2 (secp256k1 signatures)

### Off-Chain Claims (m2m token)

The system uses cryptographic claims for off-chain settlement:

1. **Claim Structure**

   ```typescript
   interface BalanceProof {
     channelId: string;
     nonce: number;
     transferredAmount: bigint;
     lockedAmount: bigint;
     locksRoot: string;
   }
   ```

2. **Signing Process**

   ```typescript
   const claimMessage = createClaimMessage(channelId, amount);
   const signature = await keyManager.sign(claimMessage, keyId);
   ```

3. **Verification**
   ```typescript
   const isValid = await verifyClaim(channelId, amount, signature, publicKey, channelAmount);
   ```

### When Settlement Occurs

- **Per-packet settlement** (if enabled): Record transfers for each forwarded packet
- **Batch settlement**: Accumulate balance proofs and settle periodically
- **Channel closure**: Final settlement when channel is cooperatively or unilaterally closed

## TigerBeetle Accounting

### Overview

The 5-peer deployment includes **TigerBeetle**, a high-performance accounting database designed for financial workloads. TigerBeetle tracks peer account balances and settlement state in real-time with ACID guarantees.

**Key Benefits:**

- High-performance: Handles millions of transactions per second
- ACID-compliant: Ensures data integrity for financial operations
- Memory-efficient: Uses minimal resources (~512MB limit)
- Persistent: Balances survive container restarts

### Architecture

```
┌──────────────┐
│  TigerBeetle │
│   (port 3000)│
└──────┬───────┘
       │ Binary Protocol (TCP)
       ▼
┌──────┴──────┐
│   Peer1-5   │
│ (Connectors)│
└─────────────┘
```

TigerBeetle runs as a single-replica cluster and is accessible to all peers via Docker networking.

### Configuration

Each peer is configured with TigerBeetle environment variables:

```yaml
environment:
  # TigerBeetle accounting configuration
  TIGERBEETLE_CLUSTER_ID: '0'
  TIGERBEETLE_REPLICAS: tigerbeetle-5peer:3000
```

| Variable                 | Description                            | Value                    |
| ------------------------ | -------------------------------------- | ------------------------ |
| `TIGERBEETLE_CLUSTER_ID` | TigerBeetle cluster identifier         | `"0"`                    |
| `TIGERBEETLE_REPLICAS`   | TigerBeetle server address (host:port) | `tigerbeetle-5peer:3000` |

### Initialization

TigerBeetle requires a one-time initialization before first use. The deployment script handles this automatically:

```bash
# Manual initialization (if needed)
docker volume create tigerbeetle-5peer-data
docker run --rm -v tigerbeetle-5peer-data:/data tigerbeetle/tigerbeetle:latest \
  format --cluster=0 --replica=0 --replica-count=1 /data/0_0.tigerbeetle
```

**Important:** The cluster ID (0) and replica settings are immutable after initialization.

### Health Check

TigerBeetle uses a TCP socket check (no HTTP endpoint):

```yaml
healthcheck:
  test: ['CMD-SHELL', 'nc -z localhost 3000 || exit 1']
  interval: 10s
  timeout: 5s
  retries: 5
  start_period: 10s
```

Check TigerBeetle health manually:

```bash
docker inspect --format='{{.State.Health.Status}}' tigerbeetle-5peer
```

### Data Persistence

TigerBeetle data is stored in a Docker volume:

- **Volume Name:** `tigerbeetle-5peer-data`
- **Mount Path:** `/data` inside container
- **Data File:** `0_0.tigerbeetle`

**Persistence behavior:**

- Balances persist across container restarts
- Data is removed with `docker-compose down -v`
- Volume can be backed up with `docker run --rm -v tigerbeetle-5peer-data:/data -v $(pwd):/backup alpine tar -czvf /backup/tigerbeetle-backup.tar.gz /data`

### Troubleshooting TigerBeetle

**TigerBeetle not starting:**

1. Check if volume exists: `docker volume inspect tigerbeetle-5peer-data`
2. Check if data file is initialized: `docker run --rm -v tigerbeetle-5peer-data:/data alpine ls -la /data/`
3. Re-initialize if needed (see Initialization section above)

**Peers cannot connect to TigerBeetle:**

1. Verify TigerBeetle is healthy: `docker inspect --format='{{.State.Health.Status}}' tigerbeetle-5peer`
2. Test connectivity: `docker exec peer1 nc -zv tigerbeetle-5peer 3000`
3. Check Docker network: `docker network inspect docker-compose-5-peer-multihop_ilp-network`

**Data corruption or reset:**

1. Stop all containers: `docker-compose -f docker-compose-5-peer-multihop.yml down`
2. Remove volume: `docker volume rm tigerbeetle-5peer-data`
3. Re-deploy: `./scripts/deploy-5-peer-multihop.sh`

## Troubleshooting

### Peer Not Starting

**Symptom:** Peer container exits or restarts repeatedly

**Solutions:**

1. Check logs: `docker-compose -f docker-compose-5-peer-multihop.yml logs peer2`
2. Verify configuration file exists: `ls -la examples/multihop-peer2.yaml`
3. Check environment variables: `docker-compose -f docker-compose-5-peer-multihop.yml config`

### BTP Connection Failed

**Symptom:** Logs show "btp_connection_error" or "btp_auth_error"

**Solutions:**

1. Verify peer is running: `docker-compose -f docker-compose-5-peer-multihop.yml ps`
2. Check auth tokens match between client and server
3. Verify network connectivity: `docker exec peer2 ping peer1`

### Packet Rejected

**Symptom:** Packet returns REJECT instead of FULFILL

**Common Error Codes:**

- `F02_UNREACHABLE`: No route to destination (check routing tables)
- `R00_TRANSFER_TIMED_OUT`: Packet expired (increase expiry time)
- `T01_PEER_UNREACHABLE`: Cannot reach next-hop peer (check BTP connection)
- `T04_INSUFFICIENT_LIQUIDITY`: Credit limit exceeded (fund peer or open channel)

### Treasury Funding Failed

**Symptom:** "Failed to send ETH to peer" error

**Solutions:**

1. Check treasury wallet balance: `cast balance <TREASURY_ADDRESS> --rpc-url http://localhost:8545`
2. Verify RPC connection: `curl -X POST http://localhost:8545 -H 'Content-Type: application/json' -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}'`
3. Check private key is correctly formatted (0x prefix)

## Advanced Usage

### Custom Number of Peers

To deploy a different number of peers, modify:

1. **Docker Compose**: Add/remove peer services
2. **Config Files**: Create YAML files for new peers
3. **Routing Tables**: Update routes to reflect new topology
4. **Deployment Script**: Update peer count and verification logic

### Different Topologies

The linear chain can be adapted to other topologies:

- **Hub-Spoke**: Central hub with multiple spoke peers
- **Mesh**: Full mesh where each peer connects to all others
- **Tree**: Hierarchical tree structure with multiple levels

See existing examples:

- `docker-compose-hub-spoke.yml` - Hub-spoke topology
- `docker-compose-mesh.yml` - Full mesh topology

### Settlement Configuration

Enable per-packet settlement by adding to peer config:

```yaml
settlement:
  enabled: true
  settlementThreshold: 1000000 # Settle after 1M units transferred
  initialDepositMultiplier: 10 # Initial deposit = threshold × 10
```

### Custom ILP Addresses

Change ILP addresses by modifying peer config files:

```yaml
# Instead of g.peer1
ilpAddress: g.mynetwork.connector1
```

And update routing tables accordingly.

## Reference

### Files

- `scripts/deploy-5-peer-multihop.sh` - Automated deployment script
- `docker-compose-5-peer-multihop.yml` - Docker Compose configuration
- `examples/multihop-peer*.yaml` - Peer configuration files
- `tools/fund-peers/` - Treasury funding utility
- `tools/send-packet/` - Packet sending utility

### Environment Variables

| Variable                   | Description                 | Example                 |
| -------------------------- | --------------------------- | ----------------------- |
| `TREASURY_EVM_PRIVATE_KEY` | Treasury wallet private key | `0x...`                 |
| `BASE_L2_RPC_URL`          | Ethereum RPC endpoint       | `http://localhost:8545` |
| `PEER{N}_EVM_ADDRESS`      | Peer N Ethereum address     | `0x...`                 |
| `BTP_PEER_{NAME}_SECRET`   | BTP authentication token    | `secret-...`            |

### Port Mappings

| Service     | Port | Protocol | Container         | Description                    |
| ----------- | ---- | -------- | ----------------- | ------------------------------ |
| TigerBeetle | 3000 | TCP      | tigerbeetle-5peer | Accounting database (internal) |
| Peer1       | 3000 | WS       | peer1             | BTP server                     |
| Peer1       | 9080 | HTTP     | peer1             | Health check                   |
| Peer2       | 3001 | WS       | peer2             | BTP server                     |
| Peer2       | 9081 | HTTP     | peer2             | Health check                   |
| Peer3       | 3002 | WS       | peer3             | BTP server                     |
| Peer3       | 9082 | HTTP     | peer3             | Health check                   |
| Peer4       | 3003 | WS       | peer4             | BTP server                     |
| Peer4       | 9083 | HTTP     | peer4             | Health check                   |
| Peer5       | 3004 | WS       | peer5             | BTP server                     |
| Peer5       | 9084 | HTTP     | peer5             | Health check                   |

**Note:** TigerBeetle port (3000) is internal only and not exposed to the host.

## Next Steps

- **Production Deployment**: See `docs/operators/production-deployment.md`
- **Security Hardening**: See `docs/operators/security-hardening-guide.md`
- **Monitoring**: Set up Prometheus and Grafana (see `docker-compose-monitoring.yml`)
- **Load Testing**: Use `scripts/run-load-test.sh` for performance testing
