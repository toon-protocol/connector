# Peer Onboarding Guide

This guide explains how to join the M2M network as a connector operator, including the onboarding wizard, manual configuration, and network connectivity requirements.

## Table of Contents

1. [Overview](#overview)
2. [Using the Onboarding Wizard](#using-the-onboarding-wizard)
3. [Manual Configuration](#manual-configuration)
4. [Network Connectivity](#network-connectivity)
5. [Peer Discovery](#peer-discovery)
6. [Security Best Practices](#security-best-practices)
7. [Testing Your Connection](#testing-your-connection)

## Overview

The M2M network is a mesh of ILP connectors that route payments across different blockchains. As a connector operator, you'll need to:

1. Configure your connector with blockchain addresses
2. Set up secure key management
3. Connect to existing peers in the network
4. Optionally enable peer discovery for automatic connections

## Using the Onboarding Wizard

The onboarding wizard is the easiest way to configure your connector.

### Prerequisites

- Node.js 20+ installed
- Access to your EVM blockchain address

### Running the Wizard

```bash
# Using npx (recommended)
npx @crosstown/connector setup

# Or if installed locally
npm run setup --workspace=packages/connector
```

### Wizard Steps

The wizard will guide you through the following:

#### 1. Node ID

```
? Enter a unique node ID for this connector: (connector-a1b2c3d4)
```

Choose a unique identifier for your connector. This is used for:

- Logging and monitoring
- Peer identification
- Audit trails

#### 2. EVM Address

```
? Enter your Ethereum address (0x...): 0x742d35Cc6634C0532925a3b844Bc9e7595f12AB3
```

Enter the EVM address that will receive settlement payments on Base L2:

- **EVM Address**: Must be `0x` followed by 40 hexadecimal characters

#### 3. Key Management

```
? Select your key management backend:
  ❯ Environment variables (development only)
    AWS KMS (production)
    GCP KMS (production)
    Azure Key Vault (production)
```

**IMPORTANT**: For production, always use a cloud KMS service.

#### 4. Monitoring

```
? Enable Prometheus/Grafana monitoring? (Y/n)
```

Recommended to enable for production visibility.

#### 5. Network Ports

```
? BTP server port: (4000)
? Health check and metrics HTTP port: (8080)
```

Default ports work for most deployments.

#### 6. Log Level

```
? Select log level:
    debug - Verbose debugging information
  ❯ info - General operational information (recommended)
    warn - Warning messages only
    error - Error messages only
```

### Output

The wizard generates a `.env` file with your configuration:

```bash
# Configuration saved to: /path/to/project/.env
```

## Manual Configuration

If you prefer manual configuration, copy and edit the environment template:

```bash
cp .env.example .env
```

### Required Settings

```bash
# Node identity
NODE_ID=my-connector

# Blockchain RPC endpoint
BASE_RPC_URL=https://mainnet.base.org

# Your settlement address
EVM_ADDRESS=0x742d35Cc6634C0532925a3b844Bc9e7595f12AB3

# Key management (NEVER use 'env' in production!)
KEY_BACKEND=aws-kms
AWS_REGION=us-east-1
AWS_KMS_EVM_KEY_ID=arn:aws:kms:...
```

### Peer Configuration

Edit `examples/production-single-node.yaml`:

```yaml
nodeId: my-connector
ilpAddress: g.connector.myconnector

peers:
  # Upstream peer (parent relationship)
  - id: upstream-hub
    relation: parent
    btpUrl: ws://hub.example.com:4000
    maxPacketAmount: 1000000000000

  # Downstream peer (child relationship)
  - id: downstream-merchant
    relation: child
    btpUrl: ws://merchant.example.com:4000

  # Symmetric peer (sibling relationship)
  - id: partner-connector
    relation: peer
    btpUrl: ws://partner.example.com:4000
```

## Network Connectivity

### Required Outbound Access

| Endpoint        | Port          | Purpose         |
| --------------- | ------------- | --------------- |
| Base L2 RPC     | 443           | EVM blockchain  |
| Peer connectors | 4000 (varies) | BTP connections |

### Required Inbound Access

| Port | Purpose                   |
| ---- | ------------------------- |
| 4000 | BTP WebSocket server      |
| 8080 | Health checks and metrics |

### Firewall Configuration

```bash
# Allow inbound BTP connections
sudo ufw allow 4000/tcp

# Allow health check access (optional, for monitoring)
sudo ufw allow 8080/tcp
```

## Peer Discovery

Peer discovery allows automatic connection to other connectors in the network.

### Enabling Discovery

In your `.env` file:

```bash
PEER_DISCOVERY_ENABLED=true
PEER_DISCOVERY_ENDPOINTS=http://discovery.m2m.network:9999
PEER_ANNOUNCE_ADDRESS=ws://my-connector.example.com:4000
```

### How It Works

1. Your connector announces itself to discovery endpoints
2. Other connectors discover your presence
3. Connections are established automatically
4. Peers are tracked and reconnected if connections drop

### Discovery Configuration Options

| Variable                   | Default     | Description                            |
| -------------------------- | ----------- | -------------------------------------- |
| `PEER_DISCOVERY_ENABLED`   | `false`     | Enable/disable discovery               |
| `PEER_DISCOVERY_ENDPOINTS` | -           | Comma-separated discovery service URLs |
| `PEER_ANNOUNCE_ADDRESS`    | auto-detect | Public WebSocket URL to announce       |

### Running Your Own Discovery Service

For private networks, you can run your own discovery service. Contact the M2M team for the discovery service software.

## Security Best Practices

### Key Management

1. **Never use `KEY_BACKEND=env` in production**
   - Environment variables can be leaked through process listings
   - Use cloud KMS for proper key protection

2. **Rotate keys regularly**
   - Configure automatic key rotation in your KMS
   - Test key rotation in staging first

3. **Use separate keys per environment**
   - Development, staging, and production should use different keys

### Network Security

1. **Use TLS for peer connections** (when supported)
2. **Restrict management port access** (8080)
3. **Monitor for unusual traffic patterns**

### Authentication

#### Authenticated Connections (Recommended)

For authenticated peer connections, configure shared secrets:

```bash
# In .env
BTP_PEER_PARTNER_SECRET=your-shared-secret-here
```

Generate strong secrets:

```bash
openssl rand -base64 32
```

Each peer connection can have its own secret. The environment variable format is:

```bash
BTP_PEER_<PEER_ID>_SECRET=secret-value
```

Where `<PEER_ID>` is the peer's node ID in uppercase with hyphens replaced by underscores.

Example:

```bash
# For peer with id "connector-upstream"
BTP_PEER_CONNECTOR_UPSTREAM_SECRET=abc123xyz789
```

#### No-Auth Connections (Permissionless Networks) - DEFAULT

Per RFC-0023, BTP supports unauthenticated connections using an empty auth token. This is the **default configuration** and is recommended for permissionless, ILP-gated networks.

**Network Architecture:**

In a permissionless network, access control happens at the **ILP layer** (via routing policies, rate limits, and settlement rules), not at the BTP transport layer. This separates concerns:

- **BTP (Transport)**: Provides reliable packet delivery between peers
- **ILP (Application)**: Enforces access control, routing policies, and economic incentives

**No-auth mode is enabled by default.** To disable it (for private networks):

```bash
# In .env - only needed for private networks with authenticated BTP
BTP_ALLOW_NOAUTH=false
```

By default, any peer can connect to your BTP server without authentication. The server still requires a peer ID for tracking and routing purposes.

**When to use no-auth:**

- ✅ **Permissionless public networks** (access control via ILP routing/settlement)
- ✅ Local development and testing
- ✅ Networks where economic incentives prevent abuse
- ✅ ILP-gated networks with rate limiting and credit controls

**When to use authenticated BTP:**

- ✅ **Private networks** with known, trusted peers
- ✅ Networks requiring transport-layer access control
- ✅ Bilateral relationships with pre-arranged settlement terms

**Client configuration for no-auth:**

In your peer configuration YAML:

```yaml
peers:
  - id: test-peer
    relation: peer
    btpUrl: ws://localhost:4000
    authToken: '' # Empty string for no-auth (requires BTP_ALLOW_NOAUTH=true on server)
```

Or using the BTP client directly:

```typescript
const peer: Peer = {
  id: 'test-peer',
  url: 'ws://localhost:4000',
  authToken: '', // Empty for no-auth
  connected: false,
  lastSeen: new Date(),
};
```

### ILP-Layer Gating (Production Security)

When running a permissionless network with no-auth BTP, security is enforced at the ILP layer through:

#### 1. Routing Policies

Control which ILP addresses can be reached through your connector:

```yaml
routes:
  # Only route to known prefixes
  - prefix: g.peer-a
    nextHop: peer-a
  - prefix: g.peer-b
    nextHop: peer-b
  # Block all other destinations (implicit deny)
```

#### 2. Credit Limits and Settlement

Configure per-peer credit limits to prevent abuse:

```bash
# Environment variables
DEFAULT_CREDIT_LIMIT=1000000  # 1M units default credit
SETTLEMENT_THRESHOLD=500000    # Settle at 50% credit usage
```

Peers must settle on-chain before exceeding credit limits, providing economic security.

#### 3. Rate Limiting

Configure packet rate limits per peer (coming soon):

```yaml
peers:
  - id: unknown-peer
    maxPacketsPerSecond: 100 # Prevent DoS attacks
    maxPacketAmount: 1000000 # Cap individual packet size
```

#### 4. Payment Channel Requirements

Require peers to open payment channels before routing:

```bash
REQUIRE_PAYMENT_CHANNELS=true
MIN_CHANNEL_CAPACITY=10000000  # Minimum 10M units per channel
```

Peers must lock capital in payment channels, providing economic security without BTP authentication.

#### 5. Network-Level Protection

Additional protections at the infrastructure level:

```bash
# Rate limiting at WebSocket server
BTP_MAX_CONNECTIONS_PER_IP=10
BTP_CONNECTION_RATE_LIMIT=5/minute

# Connection limits
BTP_MAX_TOTAL_CONNECTIONS=1000
```

**Production Checklist for Permissionless Networks:**

- ✅ Enable `BTP_ALLOW_NOAUTH=true`
- ✅ Configure credit limits per peer
- ✅ Set settlement thresholds
- ✅ Require payment channel deposits
- ✅ Implement routing policies (allowlist or denylist)
- ✅ Enable connection rate limiting
- ✅ Monitor peer behavior and adjust limits
- ✅ Set up alerts for suspicious activity

## Testing Your Connection

### 1. Verify Health

```bash
curl http://localhost:8080/health
```

Expected response:

```json
{
  "status": "healthy",
  "dependencies": {
    "tigerbeetle": { "status": "connected" },
    "evm": { "status": "connected" }
  }
}
```

### 2. Check Peer Connections

```bash
curl http://localhost:8080/health | jq .peers
```

### 3. Verify Routing

Send a test packet through the network using the CLI tools:

```bash
npx @crosstown/connector health --url http://localhost:8080/health
```

### 4. Monitor Metrics

Check Prometheus metrics for successful packet routing:

```bash
curl http://localhost:8080/metrics | grep ilp_packets
```

## Common Issues

### "No peers connected"

- Verify peer URLs in configuration
- Check firewall allows outbound WebSocket connections
- Confirm peer is online and accepting connections

### "Settlement address invalid"

- EVM addresses must be 42 characters (0x + 40 hex)

### "Key management backend error"

- Verify IAM permissions for KMS access
- Check key IDs are correct
- Ensure region matches key location

## Getting Help

- **Documentation**: See [production-deployment-guide.md](production-deployment-guide.md)
- **Issues**: https://github.com/m2m-network/m2m/issues
- **Monitoring**: See [monitoring-setup-guide.md](monitoring-setup-guide.md)
