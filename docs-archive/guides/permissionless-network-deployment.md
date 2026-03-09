# Permissionless Network Deployment Guide

## Overview

This guide explains how to deploy a connector node in a **permissionless, ILP-gated network** where:

- **BTP layer** provides open transport (anyone can connect via WebSocket)
- **ILP layer** enforces access control, economic security, and anti-abuse protections
- **Settlement layer** provides cryptographic and economic guarantees through payment channels

This architecture separates concerns and enables truly permissionless networks with robust security.

## Architecture: Layered Security Model

```
┌─────────────────────────────────────────────────────────────┐
│  Application Layer (Your Agent/Service)                     │
│  - Business logic                                            │
│  - Payment handling                                          │
└─────────────────────────────────────────────────────────────┘
                           ▲
                           │ ILP Packets
                           ▼
┌─────────────────────────────────────────────────────────────┐
│  ILP Layer (Access Control & Economic Security)             │
│  ✅ Routing policies (allowlist/denylist)                   │
│  ✅ Credit limits per peer                                  │
│  ✅ Settlement thresholds                                   │
│  ✅ Payment channel requirements                            │
│  ✅ Rate limiting                                           │
└─────────────────────────────────────────────────────────────┘
                           ▲
                           │ Authenticated ILP Packets
                           ▼
┌─────────────────────────────────────────────────────────────┐
│  BTP Layer (Open Transport)                                 │
│  ⚡ WebSocket connections (no authentication)               │
│  ⚡ Reliable packet delivery                                │
│  ⚡ Permissionless connectivity                             │
└─────────────────────────────────────────────────────────────┘
                           ▲
                           │ WebSocket
                           ▼
┌─────────────────────────────────────────────────────────────┐
│  Settlement Layer (On-Chain Security)                       │
│  🔒 Payment channels on Base L2 (EVM)                       │
│  🔒 Locked capital provides economic security               │
│  🔒 Cryptographic settlement enforcement                    │
└─────────────────────────────────────────────────────────────┘
```

## Quick Start Configuration

### 1. No-Auth BTP (Default)

**No configuration needed** - permissionless BTP is enabled by default.

To explicitly disable (for private networks):

```bash
# Only needed for private networks with BTP authentication
# BTP_ALLOW_NOAUTH=false
```

### 2. Configure ILP-Layer Protections

```bash
# Credit limits (primary defense against abuse)
SETTLEMENT_GLOBAL_CREDIT_CEILING=1000000  # Max 1M units credit per peer

# Settlement enforcement
SETTLEMENT_THRESHOLD=500000  # Settle at 50% credit usage

# Payment channel requirements
REQUIRE_PAYMENT_CHANNELS=true
MIN_CHANNEL_CAPACITY=10000000  # 10M units minimum channel deposit
```

### 3. Configure Peer Connections

In your `config.yaml`:

```yaml
nodeId: my-connector
ilpAddress: g.connector.mynode

peers:
  # Permissionless peers (no authToken required)
  - id: peer-a
    relation: peer
    btpUrl: ws://peer-a.network.com:4000
    authToken: '' # Empty for no-auth
    maxPacketAmount: 1000000

  - id: peer-b
    relation: peer
    btpUrl: ws://peer-b.network.com:4000
    authToken: '' # Empty for no-auth
    maxPacketAmount: 1000000

routes:
  # Routing policies control which destinations are reachable
  - prefix: g.peer-a
    nextHop: peer-a
  - prefix: g.peer-b
    nextHop: peer-b
```

### 4. Network-Level Protection (Optional)

```bash
# Connection rate limiting
BTP_MAX_CONNECTIONS_PER_IP=10
BTP_CONNECTION_RATE_LIMIT=5/minute
BTP_MAX_TOTAL_CONNECTIONS=1000
```

## Security Layers Explained

### Layer 1: BTP Transport (Permissionless)

**Purpose:** Provide reliable packet delivery without gatekeeping

**Configuration:**

```bash
# Default - no configuration needed
# Permissionless mode enabled by default
```

To switch to private network mode:

```bash
BTP_ALLOW_NOAUTH=false
```

**Security Model:**

- ✅ Anyone can connect via WebSocket (default)
- ✅ Peer ID required for routing (but not authenticated)
- ✅ No economic barrier to connectivity
- ❌ No access control at this layer

**Rationale:** Keeps the network truly permissionless. Abuse prevention happens at higher layers.

### Layer 2: ILP Routing (Policy Control)

**Purpose:** Control which destinations are reachable through your connector

**Configuration:**

```yaml
routes:
  - prefix: g.peer-a # ONLY route to known peers
    nextHop: peer-a
  - prefix: g.peer-b
    nextHop: peer-b
  # Implicit deny: all other destinations unreachable
```

**Security Model:**

- ✅ Explicit allowlist of reachable destinations
- ✅ Prevents your connector from becoming an open relay
- ✅ You control which parts of the network you connect
- ❌ Doesn't prevent abuse from allowed peers

### Layer 3: Credit Limits (Economic Gating)

**Purpose:** Limit exposure to counterparty risk

**Configuration:**

```bash
SETTLEMENT_GLOBAL_CREDIT_CEILING=1000000
SETTLEMENT_THRESHOLD=500000
```

**Security Model:**

- ✅ Peers can only build up limited debt before packets rejected
- ✅ Forces periodic settlement (ILP payment → on-chain settlement)
- ✅ Caps maximum loss per peer
- ✅ Works automatically without manual intervention

**Example Flow:**

1. Peer A forwards packets worth 400k units through your connector
2. Your accounting system tracks: Peer A owes 400k
3. Peer A reaches settlement threshold (500k)
4. Your connector automatically settles via payment channel
5. Peer A's debt reset to 0, can continue forwarding packets

### Layer 4: Payment Channels (Capital Lockup)

**Purpose:** Economic security through locked capital

**Configuration:**

```bash
REQUIRE_PAYMENT_CHANNELS=true
MIN_CHANNEL_CAPACITY=10000000
```

**Security Model:**

- ✅ Peers must lock capital in on-chain payment channels
- ✅ Provides cryptographic settlement guarantees
- ✅ Economic disincentive to abuse (capital at risk)
- ✅ Instant settlement without blockchain confirmations

**Example:**

- Peer must deposit 10M units to Base L2 payment channel contract
- This capital is locked and controlled by smart contract
- Connector can claim funds via cryptographic proofs
- If peer misbehaves, they lose locked capital

### Layer 5: Rate Limiting (Anti-DoS)

**Purpose:** Prevent resource exhaustion attacks

**Configuration:**

```bash
BTP_MAX_CONNECTIONS_PER_IP=10
BTP_CONNECTION_RATE_LIMIT=5/minute
BTP_MAX_TOTAL_CONNECTIONS=1000
```

**Security Model:**

- ✅ Limits connections per IP address
- ✅ Prevents single attacker from exhausting resources
- ✅ Operates at WebSocket layer (cheap to enforce)

## Production Deployment Checklist

### Essential Security (Minimum)

- [ ] `BTP_ALLOW_NOAUTH=true` enabled
- [ ] `SETTLEMENT_GLOBAL_CREDIT_CEILING` configured (e.g., 1M units)
- [ ] `SETTLEMENT_THRESHOLD` configured (e.g., 500k units)
- [ ] Routing policies defined (explicit prefixes only)
- [ ] Payment channel contracts deployed to Base L2

### Recommended Security (Production)

- [ ] `REQUIRE_PAYMENT_CHANNELS=true`
- [ ] `MIN_CHANNEL_CAPACITY` set appropriately (e.g., 10M units)
- [ ] Connection rate limiting enabled
- [ ] Per-peer packet rate limits (coming soon)
- [ ] Monitoring and alerting configured

### Operational Excellence

- [ ] Prometheus metrics enabled
- [ ] Grafana dashboards deployed
- [ ] Alerting for unusual peer behavior
- [ ] Settlement monitoring and alerts
- [ ] Payment channel balance monitoring
- [ ] Health check endpoints configured
- [ ] Backup and recovery procedures documented

## Monitoring and Alerting

### Key Metrics to Monitor

```bash
# Check peer balances (credit extended)
curl http://localhost:8080/admin/balances/:peerId

# Check payment channel states
curl http://localhost:8080/admin/channels

# Check routing table
curl http://localhost:8080/admin/routes

# Check connected peers
curl http://localhost:8080/admin/peers
```

### Critical Alerts

1. **Peer approaching credit limit**
   - Alert when peer reaches 80% of credit ceiling
   - Indicates potential settlement issues

2. **Payment channel balance low**
   - Alert when channel balance < 20% capacity
   - May need to rebalance or add liquidity

3. **Settlement failures**
   - Alert on repeated settlement claim rejections
   - Indicates blockchain issues or invalid claims

4. **Unusual traffic patterns**
   - Alert on sudden traffic spikes from single peer
   - May indicate attack or misconfiguration

## Comparison: Private vs Permissionless Networks

| Aspect             | Private Network                | Permissionless Network     |
| ------------------ | ------------------------------ | -------------------------- |
| **BTP Auth**       | Required (`authToken: secret`) | Disabled (`authToken: ""`) |
| **Access Control** | BTP layer (shared secrets)     | ILP layer (credit/routing) |
| **Onboarding**     | Manual (exchange secrets)      | Automatic (just connect)   |
| **Trust Model**    | Bilateral trust                | Economic security          |
| **Settlement**     | Pre-arranged terms             | On-chain enforcement       |
| **Use Case**       | Enterprise/private             | Public networks            |
| **Network Effect** | Limited to known peers         | Open growth                |

## Troubleshooting

### "No-auth mode disabled" error

**Problem:** Server rejects empty auth tokens

**Solution:** Enable permissionless mode:

```bash
BTP_ALLOW_NOAUTH=true
```

### Peers rejected with "Insufficient Liquidity"

**Problem:** Peer exceeded credit limit

**Solutions:**

1. Increase `SETTLEMENT_GLOBAL_CREDIT_CEILING`
2. Lower `SETTLEMENT_THRESHOLD` for faster settlement
3. Ensure peer has active payment channel with sufficient balance

### Payment channel settlement fails

**Problem:** On-chain settlement claims rejected

**Solutions:**

1. Verify payment channel contract address correct
2. Check blockchain RPC connectivity
3. Ensure sufficient gas for settlement transactions
4. Verify channel has sufficient balance

### Packets rejected for unknown destinations

**Problem:** Routing table incomplete

**Solution:** Add route for destination prefix:

```yaml
routes:
  - prefix: g.destination
    nextHop: peer-id
```

## Additional Resources

- [RFC-0023 Bilateral Transfer Protocol](../../docs/operators/peer-onboarding-guide.md)
- [ILP Routing Guide](./ilp-routing.md)
- [Production Deployment Guide](../operators/production-deployment-guide.md)
- [Security Hardening Guide](../operators/security-hardening-guide.md)

## Support

- GitHub Issues: https://github.com/m2m-network/m2m/issues
- Documentation: https://docs.m2m.network
- Community Discord: https://discord.gg/m2m-network
