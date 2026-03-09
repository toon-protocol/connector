# Production Deployment Guide

This guide provides step-by-step instructions for deploying the M2M ILP Connector in a production environment using Docker Compose.

## Table of Contents

1. [System Requirements](#system-requirements)
2. [Prerequisites](#prerequisites)
3. [Quick Start](#quick-start)
4. [Detailed Installation](#detailed-installation)
5. [Configuration](#configuration)
6. [Starting the Stack](#starting-the-stack)
7. [Verifying Deployment](#verifying-deployment)
8. [Monitoring](#monitoring)
9. [Maintenance](#maintenance)
10. [Troubleshooting](#troubleshooting)

## System Requirements

### Hardware Requirements

| Component | Minimum   | Recommended |
| --------- | --------- | ----------- |
| CPU       | 2 cores   | 4+ cores    |
| RAM       | 4 GB      | 8+ GB       |
| Storage   | 20 GB SSD | 100+ GB SSD |
| Network   | 100 Mbps  | 1 Gbps      |

### Software Requirements

- **Operating System**: Linux (Ubuntu 22.04+, Debian 11+, or similar)
- **Docker Engine**: 24.0+ (with Docker Compose plugin)
- **Docker Compose**: 2.24+

### Network Requirements

- Outbound access to blockchain RPC endpoints (Base L2)
- Inbound port for BTP connections (default: 4000)
- Inbound port for health checks (default: 8080)

## Prerequisites

### 1. Install Docker

```bash
# Ubuntu/Debian
curl -fsSL https://get.docker.com | sh
sudo usermod -aG docker $USER
# Log out and back in for group changes to take effect
```

### 2. Verify Docker Installation

```bash
docker --version
docker compose version
```

### 3. Clone the Repository

```bash
git clone https://github.com/m2m-network/m2m.git
cd m2m
```

## Quick Start

For experienced operators, here's the fastest path to deployment:

```bash
# 1. Run the onboarding wizard
npx @crosstown/connector setup

# 2. Initialize TigerBeetle (one-time)
docker run --rm -v tigerbeetle-data:/data tigerbeetle/tigerbeetle \
  format --cluster=0 --replica=0 --replica-count=1 /data/0_0.tigerbeetle

# 3. Build the connector image
docker build -t agent-runtime/connector:latest .

# 4. Start all services
docker compose -f docker-compose-production.yml up -d

# 5. Check health
curl http://localhost:8080/health
```

## Detailed Installation

### Step 1: Configure Environment

#### Option A: Interactive Wizard (Recommended)

Run the onboarding wizard to generate your configuration:

```bash
npx @crosstown/connector setup
```

The wizard will prompt you for:

- Node ID (unique identifier for your connector)
- EVM blockchain address
- Key management backend
- Monitoring preferences

#### Option B: Manual Configuration

Copy and edit the environment template:

```bash
cp .env.example .env
nano .env
```

Required settings:

```bash
NODE_ID=my-production-connector
BASE_RPC_URL=https://mainnet.base.org
EVM_ADDRESS=0x...  # Your Ethereum address
KEY_BACKEND=aws-kms  # Use KMS in production!
```

### Step 2: Configure Key Management

**IMPORTANT**: Never use `KEY_BACKEND=env` in production. Use a cloud KMS service.

#### AWS KMS

```bash
KEY_BACKEND=aws-kms
AWS_REGION=us-east-1
AWS_KMS_EVM_KEY_ID=arn:aws:kms:us-east-1:123456789012:key/...
```

#### GCP KMS

```bash
KEY_BACKEND=gcp-kms
GCP_PROJECT_ID=my-project
GCP_LOCATION_ID=us-east1
GCP_KEY_RING_ID=connector-keyring
GCP_KMS_EVM_KEY_ID=evm-signing-key
```

#### Azure Key Vault

```bash
KEY_BACKEND=azure-kv
AZURE_VAULT_URL=https://my-vault.vault.azure.net
AZURE_EVM_KEY_NAME=evm-signing-key
```

### Step 3: Initialize TigerBeetle

TigerBeetle requires a one-time initialization before first use:

```bash
docker run --rm -v tigerbeetle-data:/data tigerbeetle/tigerbeetle \
  format --cluster=0 --replica=0 --replica-count=1 /data/0_0.tigerbeetle
```

### Step 4: Build the Connector Image

```bash
docker build -t agent-runtime/connector:latest .
```

### Step 5: Configure Peer Connections

Edit `examples/production-single-node.yaml` with your peer configuration:

```yaml
nodeId: my-production-connector
ilpAddress: g.connector.mynode

peers:
  - id: upstream-peer
    relation: parent
    btpUrl: ws://upstream.example.com:4000
    # Add shared secret if using authenticated connections
```

## Starting the Stack

### Start All Services

```bash
docker compose -f docker-compose-production.yml up -d
```

### Start with Distributed Tracing

```bash
docker compose -f docker-compose-production.yml --profile tracing up -d
```

### View Logs

```bash
# All services
docker compose -f docker-compose-production.yml logs -f

# Specific service
docker compose -f docker-compose-production.yml logs -f connector
```

## Verifying Deployment

### Check Service Status

```bash
docker compose -f docker-compose-production.yml ps
```

All services should show `healthy` status.

### Check Health Endpoints

```bash
# Basic health check
curl http://localhost:8080/health

# Readiness probe (for load balancers)
curl http://localhost:8080/health/ready

# Liveness probe (for orchestrators)
curl http://localhost:8080/health/live
```

### Check Metrics

```bash
curl http://localhost:8080/metrics
```

## Monitoring

### Service Endpoints

| Service           | URL                           | Purpose              |
| ----------------- | ----------------------------- | -------------------- |
| Connector Health  | http://localhost:8080/health  | Health status        |
| Connector Metrics | http://localhost:8080/metrics | Prometheus metrics   |
| Prometheus        | http://localhost:9090         | Metrics database     |
| Grafana           | http://localhost:3001         | Dashboards           |
| Jaeger            | http://localhost:16686        | Tracing (if enabled) |

### Grafana Dashboards

Default login: `admin` / `admin` (change this in production!)

Pre-configured dashboards:

- **Connector Overview**: High-level metrics and status
- **Connector Health**: Dependency status and SLA metrics
- **Settlement Activity**: Settlement transactions and latency

### Setting Up Alerts

See `docs/operators/monitoring-setup-guide.md` for detailed alerting configuration.

## Maintenance

### Backup

```bash
# Stop services
docker compose -f docker-compose-production.yml stop

# Backup volumes
docker run --rm -v tigerbeetle-data:/data -v $(pwd)/backup:/backup \
  alpine tar czf /backup/tigerbeetle-backup.tar.gz /data

# Restart services
docker compose -f docker-compose-production.yml start
```

### Updates

```bash
# Pull latest code
git pull

# Rebuild connector image
docker build -t agent-runtime/connector:latest .

# Restart with new image
docker compose -f docker-compose-production.yml up -d
```

### CI/CD Automated Deployment

The recommended way to deploy updates is through the CI/CD pipeline:

1. **Automatic Staging Deployment**: Merging to `main` triggers automatic deployment to staging
2. **Manual Production Deployment**: Use GitHub Actions workflow dispatch for production

#### Triggering a Deployment

```bash
# Via GitHub CLI
gh workflow run cd.yml -f environment=staging -f image_tag=latest

# For production (requires environment approval)
gh workflow run cd.yml -f environment=production -f image_tag=v1.2.3
```

#### Required GitHub Secrets

Configure these secrets in your repository settings:

| Secret                | Description                           |
| --------------------- | ------------------------------------- |
| `STAGING_HOST`        | Staging server hostname or IP         |
| `STAGING_SSH_KEY`     | SSH private key for staging server    |
| `STAGING_USERNAME`    | SSH username (default: `deploy`)      |
| `PRODUCTION_HOST`     | Production server hostname or IP      |
| `PRODUCTION_SSH_KEY`  | SSH private key for production server |
| `PRODUCTION_USERNAME` | SSH username (default: `deploy`)      |

### Rollback Procedures

If a deployment causes issues, you can roll back to a previous version:

#### Automatic Rollback

The CD pipeline automatically attempts rollback if health checks fail post-deployment.

#### Manual Rollback

```bash
# Using the rollback script
IMAGE_NAME=ghcr.io/your-org/agent-runtime ./scripts/rollback.sh v1.2.2

# Or manually with Docker Compose
export IMAGE_TAG=v1.2.2
docker compose -f docker-compose-production.yml up -d connector

# Verify health after rollback
curl http://localhost:8080/health
```

#### Rollback Best Practices

1. **Know the previous version**: Check `.previous-tag` file in the deploy directory
2. **Verify health immediately**: Run health check after rollback
3. **Document the incident**: Record what went wrong and why rollback was needed
4. **Test in staging first**: Before production rollback, verify the old version works in staging

### Stopping Services

```bash
# Graceful stop (preserves data)
docker compose -f docker-compose-production.yml down

# Stop and remove volumes (CAUTION: data loss)
docker compose -f docker-compose-production.yml down -v
```

## Troubleshooting

### Common Issues

| Issue                 | Symptoms                      | Cause                       | Solution                                    |
| --------------------- | ----------------------------- | --------------------------- | ------------------------------------------- |
| Service won't start   | Container exits immediately   | Missing `.env` file         | Copy `.env.example` to `.env` and configure |
| Service won't start   | "TigerBeetle not formatted"   | TigerBeetle not initialized | Run format command (see Step 3)             |
| Health check failing  | `/health` returns 503         | Dependencies not ready      | Check TigerBeetle and RPC connectivity      |
| No peers connected    | `peersConnected: 0`           | Peer URLs incorrect         | Verify peer configuration in YAML           |
| TigerBeetle error     | Health shows TigerBeetle down | Data file missing           | Reinitialize TigerBeetle                    |
| Blockchain RPC errors | Settlement failures           | RPC endpoint issues         | Add backup RPC endpoints                    |
| Permission denied     | Cannot access data directory  | Wrong file ownership        | `chown -R 1000:1000 data/`                  |
| Port in use           | "EADDRINUSE" error            | Port conflict               | Change port or stop conflicting service     |
| Out of memory         | Container killed (OOM)        | Insufficient RAM            | Increase memory or reduce batch sizes       |
| KMS error             | Key management failed         | Missing IAM permissions     | Check cloud IAM configuration               |

### Service Won't Start

Check logs for errors:

```bash
docker compose -f docker-compose-production.yml logs connector
```

Common causes:

- Missing `.env` file
- TigerBeetle not initialized
- Invalid configuration
- Insufficient permissions

### Health Check Failing

```bash
# Check detailed health status
curl -s http://localhost:8080/health | jq .

# Check container health
docker inspect agent-runtime --format='{{.State.Health.Status}}'
```

### TigerBeetle Connection Issues

```bash
# Check TigerBeetle is running
docker logs m2m-tigerbeetle

# Verify TigerBeetle data file exists
docker exec m2m-tigerbeetle ls -la /data/
```

### Network Connectivity Issues

```bash
# Test blockchain RPC connectivity
curl -X POST https://mainnet.base.org \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}'
```

### Getting Help

- Check the [incident response runbook](incident-response-runbook.md)
- Review logs: `docker compose logs -f`
- File issues: https://github.com/m2m-network/m2m/issues

## Security Best Practices

1. **Never commit `.env` to version control**
2. **Use cloud KMS for key management** (not `KEY_BACKEND=env`)
3. **Change default Grafana password**
4. **Use TLS for external connections**
5. **Restrict network access** to management ports
6. **Enable audit logging** (see configuration guide)
7. **Regular security updates** for Docker and dependencies

For comprehensive security hardening, see [security-hardening-guide.md](./security-hardening-guide.md).

---

## Related Documentation

| Document                                                      | Description                                 |
| ------------------------------------------------------------- | ------------------------------------------- |
| [Upgrade and Migration Guide](./upgrade-migration-guide.md)   | Version upgrades and rollback procedures    |
| [Backup and Disaster Recovery](./backup-disaster-recovery.md) | Backup procedures and DR strategies         |
| [Security Hardening Guide](./security-hardening-guide.md)     | TLS, KMS, network security configuration    |
| [Monitoring Setup Guide](./monitoring-setup-guide.md)         | Prometheus, Grafana, alerting configuration |
| [Incident Response Runbook](./incident-response-runbook.md)   | Incident handling procedures                |
| [Performance Tuning Guide](./performance-tuning-guide.md)     | Optimization for high throughput            |
| [API Reference](./api-reference.md)                           | Health and metrics endpoint documentation   |
| [Peer Onboarding Guide](./peer-onboarding-guide.md)           | Joining the network as a peer               |

---

**Document Version**: 1.1
**Last Updated**: 2026-01-23
