# TigerBeetle Deployment Guide

## Table of Contents

- [Overview](#overview)
- [What is TigerBeetle?](#what-is-tigerbeetle)
- [Docker Deployment](#docker-deployment)
  - [Single-Node Deployment (Development)](#single-node-deployment-development)
  - [Multi-Replica Deployment (Production)](#multi-replica-deployment-production)
- [Configuration](#configuration)
  - [Environment Variables](#environment-variables)
  - [Cluster Configuration](#cluster-configuration)
- [Operations](#operations)
  - [Starting TigerBeetle](#starting-tigerbeetle)
  - [Monitoring Health](#monitoring-health)
  - [Viewing Logs](#viewing-logs)
  - [Data Persistence](#data-persistence)
- [Operational Considerations](#operational-considerations)
  - [Cluster ID Immutability](#cluster-id-immutability)
  - [Backup Strategy](#backup-strategy)
  - [Scaling](#scaling)
- [Troubleshooting](#troubleshooting)
- [References](#references)

---

## Overview

TigerBeetle is the distributed accounting database used by the M2M ILP connector for settlement layer tracking. It maintains double-entry bookkeeping ledgers for peer balances with ACID guarantees, providing the foundation for cryptocurrency payment channel settlement in Epic 6.

**Key Features:**

- High-performance distributed accounting (millions of transactions per second)
- ACID guarantees with microsecond-level latency
- Double-entry bookkeeping for settlement balances
- Quorum-based consensus for multi-replica deployments
- Immutable audit trail of all transfers

**Role in M2M System:**

TigerBeetle sits between the ILP packet forwarding layer (Epics 1-4) and the cryptocurrency settlement layer (Epic 6+). When connectors forward ILP packets, TigerBeetle records the resulting balance changes in peer accounts, enabling threshold-based settlement to blockchain payment channels.

```
┌─────────────────┐
│ ILP Connectors  │ (Packet forwarding)
└────────┬────────┘
         │
         ↓
┌─────────────────┐
│  TigerBeetle    │ (Settlement accounting)
└────────┬────────┘
         │
         ↓
┌─────────────────┐
│ Payment Channels│ (Blockchain settlement)
└─────────────────┘
```

---

## What is TigerBeetle?

**TigerBeetle** is a purpose-built distributed accounting database designed for financial systems. Unlike general-purpose databases, TigerBeetle is optimized specifically for:

- **Double-Entry Bookkeeping:** Every transaction affects exactly two accounts (debit and credit)
- **High Throughput:** Millions of transactions per second with minimal latency
- **Strong Consistency:** ACID guarantees ensure data integrity
- **Distributed Consensus:** Multi-replica clusters with quorum-based voting
- **Immutability:** Transactions cannot be modified or deleted once committed

**Why TigerBeetle for M2M Settlement?**

1. **Performance:** Can handle high-volume ILP packet flows without bottlenecks
2. **Correctness:** ACID guarantees ensure balances are always accurate
3. **Auditability:** Immutable transaction history provides complete audit trail
4. **Reliability:** Multi-replica deployment ensures high availability
5. **Simplicity:** Purpose-built for accounting eliminates complexity of general databases

**Official Documentation:** https://docs.tigerbeetle.com/

---

## Docker Deployment

TigerBeetle is deployed as a Docker container alongside ILP connector nodes. The M2M project provides two deployment configurations:

1. **Single-Node (Development):** Simple 1-replica cluster for local testing
2. **Multi-Replica (Production):** 3-5 replica cluster for high availability (documented, not automated)

### Single-Node Deployment (Development)

**Use Case:** Local development, testing, proof-of-concept deployments

**Architecture:**

```
┌─────────────────────────────┐
│   Docker Network (bridge)   │
│                             │
│  ┌──────────────┐           │
│  │ TigerBeetle  │           │
│  │  (replica 0) │           │
│  │  Port: 3000  │           │
│  └──────┬───────┘           │
│         │                   │
│  ┌──────▼───────┐           │
│  │ Connector A  │           │
│  └──────────────┘           │
│  ┌──────────────┐           │
│  │ Connector B  │           │
│  └──────────────┘           │
│  ┌──────────────┐           │
│  │ Connector C  │           │
│  └──────────────┘           │
└─────────────────────────────┘
```

**Quick Start:**

```bash
# Start all services (TigerBeetle + connectors + dashboard)
docker-compose up -d

# Check TigerBeetle status
docker-compose ps tigerbeetle

# Verify TigerBeetle is healthy
docker inspect tigerbeetle --format '{{.State.Health.Status}}'
# Expected output: healthy

# View TigerBeetle logs
docker-compose logs -f tigerbeetle
```

**Data Persistence:**

TigerBeetle data is stored in a Docker named volume: `tigerbeetle-data`

```bash
# List volumes
docker volume ls | grep tigerbeetle

# Inspect volume
docker volume inspect tigerbeetle-data

# Backup volume (recommended before major changes)
docker run --rm -v tigerbeetle-data:/data -v $(pwd):/backup \
  alpine tar czf /backup/tigerbeetle-backup.tar.gz /data
```

---

### Multi-Replica Deployment (Production)

**Use Case:** Production environments requiring high availability and fault tolerance

**Architecture:**

```
┌────────────────────────────────────────────────┐
│         Docker Network (bridge)                │
│                                                │
│  ┌──────────────┐  ┌──────────────┐  ┌───────────────┐
│  │ TigerBeetle  │  │ TigerBeetle  │  │ TigerBeetle   │
│  │  (replica 0) │  │  (replica 1) │  │  (replica 2)  │
│  │  Port: 3000  │  │  Port: 3001  │  │  Port: 3002   │
│  └──────┬───────┘  └──────┬───────┘  └──────┬────────┘
│         │                 │                  │
│         └─────────────────┼──────────────────┘
│                           │  Quorum Consensus
│                           │
│         ┌─────────────────┘
│         │
│  ┌──────▼───────┐
│  │ Connector A  │
│  └──────────────┘
│  ┌──────────────┐
│  │ Connector B  │
│  └──────────────┘
│  ┌──────────────┐
│  │ Connector C  │
│  └──────────────┘
└────────────────────────────────────────────────┘
```

**Cluster Configuration:**

- **Replica Count:** 3 or 5 replicas (odd numbers required for quorum)
- **Quorum:** Requires majority of replicas healthy (2/3 for 3 replicas, 3/5 for 5 replicas)
- **Network:** All replicas must communicate on same Docker network
- **Data Files:** Each replica has unique data file: `{cluster_id}_{replica_id}.tigerbeetle`

**Example docker-compose.yml (3-replica cluster):**

```yaml
version: '3.8'

services:
  tigerbeetle-0:
    image: ghcr.io/tigerbeetle/tigerbeetle:latest
    container_name: tigerbeetle-0
    security_opt:
      - seccomp=unconfined
    environment:
      TIGERBEETLE_CLUSTER_ID: 0
      TIGERBEETLE_REPLICA_ID: 0
      TIGERBEETLE_REPLICA_COUNT: 3
    volumes:
      - tigerbeetle-data-0:/data
    networks:
      - ilp-network
    restart: unless-stopped
    healthcheck:
      test: ['CMD-SHELL', 'nc -z localhost 3000 || exit 1']
      interval: 10s
      timeout: 5s
      retries: 5
      start_period: 30s
    command: >
      sh -c "
      if [ ! -f /data/0_0.tigerbeetle ]; then
        tigerbeetle format --cluster=0 --replica=0 --replica-count=3 /data/0_0.tigerbeetle
      fi &&
      tigerbeetle start --addresses=0.0.0.0:3000 --addresses=tigerbeetle-1:3000 --addresses=tigerbeetle-2:3000 /data/0_0.tigerbeetle
      "

  tigerbeetle-1:
    image: ghcr.io/tigerbeetle/tigerbeetle:latest
    container_name: tigerbeetle-1
    # Similar configuration with replica ID 1 and data file 0_1.tigerbeetle
    # ...

  tigerbeetle-2:
    image: ghcr.io/tigerbeetle/tigerbeetle:latest
    container_name: tigerbeetle-2
    # Similar configuration with replica ID 2 and data file 0_2.tigerbeetle
    # ...

volumes:
  tigerbeetle-data-0:
  tigerbeetle-data-1:
  tigerbeetle-data-2:

networks:
  ilp-network:
    driver: bridge
```

**Note:** Full multi-replica automation is not implemented in this story. For production deployments, manually create the docker-compose configuration above or use orchestration tools (Kubernetes, Docker Swarm).

---

## Configuration

### Environment Variables

TigerBeetle configuration is controlled via environment variables in `.env.production.example`:

| Variable                    | Default | Description                                    | Notes                                              |
| --------------------------- | ------- | ---------------------------------------------- | -------------------------------------------------- |
| `TIGERBEETLE_CLUSTER_ID`    | `0`     | Unique cluster identifier (0-255)              | **IMMUTABLE** - cannot change after initialization |
| `TIGERBEETLE_REPLICA_COUNT` | `1`     | Number of replicas in cluster (1, 3, 5, ...)   | Odd number required for quorum consensus           |
| `TIGERBEETLE_PORT`          | `3000`  | Client connection port (internal network only) | Do NOT expose to host (security risk)              |
| `TIGERBEETLE_DATA_DIR`      | `/data` | Data directory inside container                | Do not change (TigerBeetle convention)             |

**Example `.env` file:**

```bash
# Development (single-node)
TIGERBEETLE_CLUSTER_ID=0
TIGERBEETLE_REPLICA_COUNT=1
TIGERBEETLE_PORT=3000
TIGERBEETLE_DATA_DIR=/data

# Production (3-replica cluster)
TIGERBEETLE_CLUSTER_ID=0
TIGERBEETLE_REPLICA_COUNT=3
TIGERBEETLE_PORT=3000
TIGERBEETLE_DATA_DIR=/data
```

---

### Cluster Configuration

**Cluster ID:**

- **Purpose:** Uniquely identifies a TigerBeetle cluster deployment
- **Range:** 0-255
- **Immutability:** **CANNOT BE CHANGED** after cluster initialization
- **Uniqueness:** Each independent deployment must have unique cluster ID
- **Security Implication:** Changing cluster ID requires reformatting (ALL DATA LOST)

**Replica Count:**

- **Single-Node (Development):** `TIGERBEETLE_REPLICA_COUNT=1`
  - Simplest configuration
  - No redundancy (data loss if container fails)
  - Acceptable for development and testing

- **Multi-Replica (Production):** `TIGERBEETLE_REPLICA_COUNT=3` or `5`
  - High availability (survives replica failures)
  - Quorum consensus requires majority healthy (2/3 or 3/5)
  - Odd number required for majority voting
  - More replicas = higher availability but slower writes

**Port Configuration:**

- **Internal Port:** 3000 (default TigerBeetle client port)
- **Network Access:** Docker network only (ilp-network bridge)
- **Host Exposure:** **DO NOT EXPOSE** to host (security risk)
- **Connector Access:** `tigerbeetle:3000` (Docker hostname)

---

## Operations

### Starting TigerBeetle

**Start with docker-compose:**

```bash
# Start all services (recommended)
docker-compose up -d

# Start TigerBeetle only
docker-compose up -d tigerbeetle

# Start with specific compose file
docker-compose -f docker-compose-production.yml up -d
```

**Initialization Process:**

On first startup, TigerBeetle automatically:

1. Checks if data file exists: `/data/{cluster_id}_{replica_id}.tigerbeetle`
2. If not found, formats new data file with cluster configuration
3. Starts TigerBeetle server listening on port 3000

**Logs from first startup:**

```
TigerBeetle data file not found: /data/0_0.tigerbeetle
Formatting new cluster:
  Cluster ID: 0
  Replica ID: 0
  Replica Count: 1
TigerBeetle cluster formatted successfully
Starting TigerBeetle server on 0.0.0.0:3000
```

**Logs from subsequent startups:**

```
TigerBeetle data file already exists: /data/0_0.tigerbeetle
Skipping initialization (cluster already formatted)
Starting TigerBeetle server on 0.0.0.0:3000
```

---

### Monitoring Health

**Health Check Method:**

TigerBeetle uses TCP socket check (no HTTP endpoint available):

```bash
# Check health status
docker inspect tigerbeetle --format '{{.State.Health.Status}}'

# Possible statuses:
# - starting: Container starting, health check not yet run
# - healthy: Health check passing (TCP port 3000 responding)
# - unhealthy: Health check failing (TCP port 3000 not responding)
```

**Manual TCP Check:**

```bash
# From host (if netcat installed)
nc -zv localhost 3000

# From another container
docker exec connector-a nc -zv tigerbeetle 3000
```

**Monitoring Tips:**

- **Health Check Interval:** 10 seconds (configured in docker-compose.yml)
- **Retries:** 5 failed checks before marked unhealthy
- **Start Period:** 30 seconds (allows time for initialization)
- **Production Monitoring:** Consider external monitoring tools (Prometheus, Datadog)

---

### Viewing Logs

**View TigerBeetle logs:**

```bash
# Follow logs in real-time
docker-compose logs -f tigerbeetle

# View last 100 lines
docker-compose logs --tail=100 tigerbeetle

# View logs from specific time
docker-compose logs --since 30m tigerbeetle

# Use docker logs directly
docker logs tigerbeetle
docker logs -f tigerbeetle
```

**What to Monitor in Logs:**

- **Initialization:** Cluster formatting on first startup
- **Startup:** Server listening on 0.0.0.0:3000
- **Errors:** Connection failures, disk I/O errors, consensus issues
- **Performance:** Transaction latency warnings (if any)

---

### Data Persistence

**Docker Volume:**

TigerBeetle data is stored in named Docker volume: `tigerbeetle-data`

```bash
# List volumes
docker volume ls | grep tigerbeetle

# Inspect volume (see mount point, size, driver)
docker volume inspect tigerbeetle-data

# View volume mount point (requires sudo)
sudo ls -lah $(docker volume inspect tigerbeetle-data --format '{{.Mountpoint}}')
```

**Data File Structure:**

```
/data/
└── 0_0.tigerbeetle  # Format: {cluster_id}_{replica_id}.tigerbeetle
```

**Backup Data Volume:**

```bash
# Backup volume to tar.gz
docker run --rm \
  -v tigerbeetle-data:/data \
  -v $(pwd):/backup \
  alpine tar czf /backup/tigerbeetle-backup-$(date +%Y%m%d-%H%M%S).tar.gz /data

# Restore volume from tar.gz
docker run --rm \
  -v tigerbeetle-data:/data \
  -v $(pwd):/backup \
  alpine tar xzf /backup/tigerbeetle-backup-20250102-120000.tar.gz -C /
```

**Important:** Stop TigerBeetle container before backup for data consistency:

```bash
docker-compose stop tigerbeetle
# Perform backup
docker-compose start tigerbeetle
```

---

## Operational Considerations

### Cluster ID Immutability

**CRITICAL:** TigerBeetle cluster ID is **IMMUTABLE** after initialization.

**Why Immutable?**

- Cluster ID is embedded in data file format
- Changing cluster ID requires reformatting (ALL DATA LOST)
- Prevents accidental data corruption from ID conflicts

**Best Practices:**

1. **Document Cluster ID:** Record in production runbooks
2. **Unique IDs:** Use unique cluster ID for each independent deployment
3. **Never Change:** Do not modify `TIGERBEETLE_CLUSTER_ID` after first startup
4. **Migration:** If cluster ID change needed, export data, reformat, re-import (future work)

**What Happens if You Change Cluster ID:**

```bash
# Original cluster (cluster ID 0)
TIGERBEETLE_CLUSTER_ID=0
docker-compose up -d tigerbeetle
# Data file created: /data/0_0.tigerbeetle

# Change cluster ID (DANGEROUS)
TIGERBEETLE_CLUSTER_ID=1
docker-compose up -d tigerbeetle
# TigerBeetle will FAIL to start (cluster ID mismatch)
# Error: Data file cluster ID (0) does not match config (1)
```

**Recovery from Cluster ID Mismatch:**

```bash
# Option 1: Restore original cluster ID
TIGERBEETLE_CLUSTER_ID=0

# Option 2: Delete volume and reformat (DATA LOSS)
docker-compose down -v
docker volume rm tigerbeetle-data
TIGERBEETLE_CLUSTER_ID=1
docker-compose up -d tigerbeetle
```

---

### Backup Strategy

**Recommended Backup Frequency:**

- **Development:** Not required (ephemeral data acceptable)
- **Production:** Daily automated backups + pre-change snapshots

**Backup Workflow:**

```bash
#!/bin/bash
# Automated TigerBeetle backup script

BACKUP_DIR="/backups/tigerbeetle"
TIMESTAMP=$(date +%Y%m%d-%H%M%S)
BACKUP_FILE="${BACKUP_DIR}/tigerbeetle-backup-${TIMESTAMP}.tar.gz"

# Stop TigerBeetle for consistent backup
docker-compose stop tigerbeetle

# Create backup
docker run --rm \
  -v tigerbeetle-data:/data \
  -v ${BACKUP_DIR}:/backup \
  alpine tar czf /backup/tigerbeetle-backup-${TIMESTAMP}.tar.gz /data

# Restart TigerBeetle
docker-compose start tigerbeetle

# Verify backup
echo "Backup created: ${BACKUP_FILE}"
ls -lh "${BACKUP_FILE}"

# Retention: Keep last 30 days
find ${BACKUP_DIR} -name "tigerbeetle-backup-*.tar.gz" -mtime +30 -delete
```

**Backup Testing:**

```bash
# Test restore in separate environment
docker volume create tigerbeetle-data-test

docker run --rm \
  -v tigerbeetle-data-test:/data \
  -v $(pwd):/backup \
  alpine tar xzf /backup/tigerbeetle-backup-20250102-120000.tar.gz -C /

# Verify data file exists
docker run --rm -v tigerbeetle-data-test:/data alpine ls -lh /data
```

---

### Scaling

**Vertical Scaling (Increase Resources):**

- **CPU:** TigerBeetle is CPU-bound, more cores = higher throughput
- **Memory:** Increase for larger working set (frequently accessed accounts)
- **Disk:** Use fast SSD for write-ahead log performance

**Horizontal Scaling (Add Replicas):**

- **Current:** Single-node deployment (1 replica)
- **Production:** 3-5 replicas for high availability
- **Process:**
  1. Increase `TIGERBEETLE_REPLICA_COUNT` to 3
  2. Deploy 3 separate TigerBeetle containers with unique replica IDs
  3. Configure cluster addresses for all replicas
  4. Update connectors to connect to cluster (any replica)

**Performance Considerations:**

- **Single-Node:** Lowest latency (no consensus overhead)
- **Multi-Replica:** Higher availability but slower writes (consensus latency)
- **Quorum:** Requires majority of replicas healthy (2/3 or 3/5)

---

## Troubleshooting

### Container Won't Start

**Symptom:** TigerBeetle container exits immediately or fails to start

**Possible Causes:**

1. **Cluster ID Conflict:** Data file cluster ID doesn't match environment variable
2. **Permission Issues:** Data directory permissions incorrect
3. **Resource Constraints:** Insufficient memory or disk space

**Diagnosis:**

```bash
# Check container status
docker-compose ps tigerbeetle

# View logs for errors
docker-compose logs tigerbeetle

# Check data file exists
docker run --rm -v tigerbeetle-data:/data alpine ls -lh /data

# Verify cluster ID in environment
docker inspect tigerbeetle --format '{{.Config.Env}}'
```

**Solution:**

```bash
# If cluster ID mismatch, restore original cluster ID
TIGERBEETLE_CLUSTER_ID=0

# If data corruption, delete volume and reformat (DATA LOSS)
docker-compose down -v
docker volume rm tigerbeetle-data
docker-compose up -d tigerbeetle
```

---

### Health Check Failing

**Symptom:** Container running but health check shows "unhealthy"

**Possible Causes:**

1. **Port Not Listening:** TigerBeetle server not listening on port 3000
2. **Network Issues:** Docker network misconfiguration
3. **Health Check Timeout:** Container slow to start (increase start_period)

**Diagnosis:**

```bash
# Check if container is running
docker-compose ps tigerbeetle

# Check health status
docker inspect tigerbeetle --format '{{.State.Health.Status}}'

# Manual TCP check from another container
docker exec connector-a nc -zv tigerbeetle 3000

# View health check logs
docker inspect tigerbeetle --format '{{json .State.Health}}' | jq
```

**Solution:**

```bash
# Restart container
docker-compose restart tigerbeetle

# Increase health check start period in docker-compose.yml
healthcheck:
  start_period: 60s  # Increase from 30s

# Rebuild and restart
docker-compose up -d --force-recreate tigerbeetle
```

---

### Connection Refused from Connectors

**Symptom:** Connectors cannot connect to TigerBeetle on port 3000

**Possible Causes:**

1. **Network Isolation:** TigerBeetle not on same Docker network
2. **Hostname Resolution:** Docker DNS not resolving `tigerbeetle` hostname
3. **Port Mismatch:** Connector trying to connect to wrong port

**Diagnosis:**

```bash
# Verify network configuration
docker network inspect ilp-network

# Check if TigerBeetle is on network
docker inspect tigerbeetle --format '{{.NetworkSettings.Networks}}'

# Test connection from connector
docker exec connector-a nc -zv tigerbeetle 3000

# Check TigerBeetle logs for connection attempts
docker-compose logs tigerbeetle
```

**Solution:**

```bash
# Ensure both on same network in docker-compose.yml
networks:
  - ilp-network

# Restart services
docker-compose down
docker-compose up -d
```

---

### Data Corruption

**Symptom:** TigerBeetle fails to start with data file corruption errors

**Possible Causes:**

1. **Disk Failure:** Underlying storage failure
2. **Abrupt Shutdown:** Container killed during write operation
3. **Data File Tampering:** Manual modification of data file

**Diagnosis:**

```bash
# Check disk space
df -h

# View TigerBeetle error logs
docker-compose logs tigerbeetle

# Check for Docker volume errors
docker volume inspect tigerbeetle-data
```

**Solution:**

```bash
# Option 1: Restore from backup
docker-compose down
docker volume rm tigerbeetle-data
docker volume create tigerbeetle-data

docker run --rm \
  -v tigerbeetle-data:/data \
  -v $(pwd):/backup \
  alpine tar xzf /backup/tigerbeetle-backup-LATEST.tar.gz -C /

docker-compose up -d tigerbeetle

# Option 2: Reformat (DATA LOSS)
docker-compose down -v
docker volume rm tigerbeetle-data
docker-compose up -d tigerbeetle
```

---

## References

### TigerBeetle Official Resources

- **Official Website:** https://tigerbeetle.com/
- **Documentation:** https://docs.tigerbeetle.com/
- **GitHub Repository:** https://github.com/tigerbeetle/tigerbeetle
- **Docker Image:** https://github.com/tigerbeetle/tigerbeetle/pkgs/container/tigerbeetle

### M2M Project Documentation

- **Architecture Documentation:** [docs/architecture.md](../architecture.md)
- **Settlement Layer Design:** docs/architecture/settlement-layer.md (future)
- **Environment Configuration:** [.env.production.example](../../.env.production.example)
- **Docker Compose Configuration:** [docker-compose.yml](../../docker-compose.yml)

### Related Epics and Stories

- **Epic 6: Settlement Foundation & Accounting** (this story is part of Epic 6)
- **Story 6.1:** TigerBeetle Integration & Docker Deployment (this guide)
- **Story 6.2:** TigerBeetle Client Library Integration
- **Story 6.3:** Account Management for Peer Settlement
- **Story 6.4:** Packet Handler Integration for Recording Transfers

---

## Getting Help

If you encounter issues not covered in this guide:

1. **Check Logs:** `docker-compose logs -f tigerbeetle`
2. **Verify Configuration:** Review `.env` file and `docker-compose.yml`
3. **TigerBeetle Community:** https://github.com/tigerbeetle/tigerbeetle/discussions
4. **M2M Project Issues:** https://github.com/your-org/m2m/issues

---

**Last Updated:** 2026-01-02 (Story 6.1 Implementation)
