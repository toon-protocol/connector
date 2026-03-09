# Upgrade and Migration Guide

This guide provides procedures for upgrading the M2M ILP Connector to new versions, including version compatibility, upgrade procedures, rollback strategies, and data migration.

## Table of Contents

1. [Version Compatibility Matrix](#version-compatibility-matrix)
2. [Pre-Upgrade Checklist](#pre-upgrade-checklist)
3. [Upgrade Procedures](#upgrade-procedures)
4. [Rollback Procedures](#rollback-procedures)
5. [Data Migration](#data-migration)
6. [Breaking Changes](#breaking-changes)
7. [Blue-Green Deployment](#blue-green-deployment)
8. [Post-Upgrade Verification](#post-upgrade-verification)
9. [Troubleshooting](#troubleshooting)

---

## Version Compatibility Matrix

### Connector Version Compatibility

| Connector Version | Node.js | TigerBeetle | Docker Compose | Breaking Changes                             |
| ----------------- | ------- | ----------- | -------------- | -------------------------------------------- |
| 1.0.x             | 20.x    | 0.x         | 2.24+          | Initial release                              |
| 1.1.x             | 20.x    | 0.x         | 2.24+          | None                                         |
| 1.2.x             | 20.x    | 0.x         | 2.24+          | None                                         |
| 2.0.x             | 20.x+   | 0.x         | 2.24+          | See [Breaking Changes](#breaking-changes-v2) |

### Semantic Versioning

The M2M Connector follows [Semantic Versioning](https://semver.org/):

- **Major (X.0.0)**: Breaking changes requiring migration
- **Minor (1.X.0)**: New features, backward compatible
- **Patch (1.0.X)**: Bug fixes, backward compatible

### Upgrade Paths

| From Version | To Version | Migration Required | Notes                      |
| ------------ | ---------- | ------------------ | -------------------------- |
| 1.0.x        | 1.1.x      | No                 | Direct upgrade             |
| 1.1.x        | 1.2.x      | No                 | Direct upgrade             |
| 1.x.x        | 2.0.x      | Yes                | See data migration section |

---

## Pre-Upgrade Checklist

Complete all items before beginning an upgrade:

### 1. Backup Verification

- [ ] **Create full backup of TigerBeetle data**

  ```bash
  docker compose -f docker-compose-production.yml stop tigerbeetle
  docker run --rm -v tigerbeetle-data:/data -v $(pwd)/backup:/backup \
    alpine tar czf /backup/tigerbeetle-pre-upgrade-$(date +%Y%m%d).tar.gz /data
  docker compose -f docker-compose-production.yml start tigerbeetle
  ```

- [ ] **Backup connector configuration**

  ```bash
  cp .env .env.backup-$(date +%Y%m%d)
  cp examples/production-single-node.yaml examples/production-single-node.yaml.backup-$(date +%Y%m%d)
  ```

- [ ] **Backup wallet data** (if applicable)

  ```bash
  cp -r data/wallet data/wallet.backup-$(date +%Y%m%d)
  ```

- [ ] **Verify backup integrity**
  ```bash
  ls -la backup/
  tar -tzf backup/tigerbeetle-pre-upgrade-*.tar.gz | head
  ```

### 2. Health Verification

- [ ] **Check current system health**

  ```bash
  curl http://localhost:8080/health | jq .
  ```

- [ ] **Verify all peers connected**

  ```bash
  curl http://localhost:8080/health | jq '.peers'
  ```

- [ ] **Check for pending settlements**

  ```bash
  curl http://localhost:8080/metrics | grep 'settlements_pending'
  ```

- [ ] **Record current version**
  ```bash
  docker inspect agent-runtime --format='{{.Config.Image}}' > .current-version
  cat .current-version
  ```

### 3. Review Release Notes

- [ ] **Read CHANGELOG.md for target version**

  ```bash
  curl -s https://raw.githubusercontent.com/m2m-network/m2m/v1.2.0/CHANGELOG.md | head -100
  ```

- [ ] **Check for breaking changes**
- [ ] **Review new configuration options**
- [ ] **Note any required environment variable changes**

### 4. Communication

- [ ] **Notify stakeholders of maintenance window**
- [ ] **Update status page (if applicable)**
- [ ] **Ensure on-call engineer is available**

### 5. Environment Preparation

- [ ] **Verify disk space available**

  ```bash
  df -h /var/lib/docker
  # Recommend: At least 10GB free
  ```

- [ ] **Check Docker is up to date**

  ```bash
  docker --version
  docker compose version
  ```

- [ ] **Pull new image (pre-fetch to reduce downtime)**
  ```bash
  docker pull ghcr.io/your-org/agent-runtime:v1.2.0
  ```

---

## Upgrade Procedures

### Method 1: CI/CD Pipeline (Recommended)

The recommended upgrade method uses the automated CI/CD pipeline:

#### Staging Upgrade

```bash
# Trigger staging deployment via GitHub CLI
gh workflow run cd.yml -f environment=staging -f image_tag=v1.2.0

# Monitor workflow progress
gh run watch

# Verify staging health
curl https://staging.example.com/health | jq .
```

#### Production Upgrade

```bash
# After staging verification, deploy to production
gh workflow run cd.yml -f environment=production -f image_tag=v1.2.0

# This requires environment approval in GitHub
# Monitor workflow progress
gh run watch

# Verify production health
curl https://production.example.com/health | jq .
```

### Method 2: Manual Docker Compose Upgrade

For environments without CI/CD:

```bash
# Navigate to deployment directory
cd /opt/m2m

# Save current version for rollback
docker inspect agent-runtime --format='{{.Config.Image}}' | cut -d: -f2 > .previous-tag

# Stop current connector (gracefully)
docker compose -f docker-compose-production.yml stop connector

# Pull new image
docker pull ghcr.io/your-org/agent-runtime:v1.2.0

# Update image tag
export IMAGE_TAG=v1.2.0

# Start with new version
docker compose -f docker-compose-production.yml up -d connector

# Verify health
for i in {1..12}; do
  if curl -sf http://localhost:8080/health > /dev/null 2>&1; then
    echo "✓ Upgrade successful!"
    exit 0
  fi
  echo "Waiting for health check... ($i/12)"
  sleep 5
done

echo "✗ Health check failed - consider rollback"
```

### Method 3: Rolling Upgrade (Multi-Node)

For high-availability deployments with multiple connector nodes:

```bash
# Upgrade nodes one at a time
for node in connector-1 connector-2 connector-3; do
  echo "Upgrading $node..."

  # Drain traffic from node (if using load balancer)
  # Update load balancer to remove $node

  # Upgrade the node
  docker compose -f docker-compose-production.yml stop $node
  IMAGE_TAG=v1.2.0 docker compose -f docker-compose-production.yml up -d $node

  # Wait for health
  sleep 30
  curl -sf http://$node:8080/health || echo "Warning: $node health check failed"

  # Add back to load balancer
  # Update load balancer to add $node

  echo "$node upgraded"
  sleep 10  # Allow traffic to stabilize
done
```

---

## Rollback Procedures

### Automatic Rollback

The CD pipeline automatically rolls back if health checks fail post-deployment.

### Manual Rollback Using Script

```bash
# Using the rollback script (reads from .previous-tag)
IMAGE_NAME=ghcr.io/your-org/agent-runtime ./scripts/rollback.sh

# Or specify a specific version
IMAGE_NAME=ghcr.io/your-org/agent-runtime ./scripts/rollback.sh v1.1.0
```

### Manual Rollback (Docker Compose)

```bash
# Navigate to deployment directory
cd /opt/m2m

# Get previous version
PREV_TAG=$(cat .previous-tag)
echo "Rolling back to: $PREV_TAG"

# Stop current container
docker compose -f docker-compose-production.yml stop connector

# Start with previous version
export IMAGE_TAG=$PREV_TAG
docker compose -f docker-compose-production.yml up -d connector

# Verify rollback health
sleep 10
curl http://localhost:8080/health | jq .
```

### Emergency Rollback

If the connector is completely unresponsive:

```bash
# Force stop all containers
docker compose -f docker-compose-production.yml down

# Start with known good version
export IMAGE_TAG=v1.1.0
docker compose -f docker-compose-production.yml up -d

# Monitor logs
docker compose -f docker-compose-production.yml logs -f connector
```

### Rollback Best Practices

1. **Know the previous version**: Always check `.previous-tag` before rollback
2. **Verify health immediately**: Run health check within 30 seconds of restart
3. **Document the incident**: Record what went wrong and why rollback was needed
4. **Test in staging first**: Before production rollback, verify the old version works
5. **Preserve logs**: Save logs from the failed deployment for analysis
   ```bash
   docker logs agent-runtime > failed-deployment-$(date +%Y%m%d%H%M%S).log 2>&1
   ```

---

## Data Migration

### When Migration is Required

Data migration is required when:

- Upgrading to a new major version (e.g., 1.x → 2.x)
- Schema changes in TigerBeetle accounts
- Configuration format changes
- Wallet database schema updates

### Pre-Migration Steps

1. **Stop all traffic**

   ```bash
   # Update load balancer to return 503
   # Or stop accepting new connections
   ```

2. **Wait for pending operations**

   ```bash
   # Wait for pending settlements to complete
   while [ $(curl -s http://localhost:8080/metrics | grep 'settlements_pending' | awk '{print $2}') -gt 0 ]; do
     echo "Waiting for pending settlements..."
     sleep 5
   done
   ```

3. **Create verified backup**
   ```bash
   # Full backup with verification
   ./scripts/backup.sh --full --verify
   ```

### Migration Procedures

#### TigerBeetle Data Migration

```bash
# Stop connector
docker compose -f docker-compose-production.yml stop connector

# Export TigerBeetle data (if required by release notes)
docker exec tigerbeetle tigerbeetle-export /data/export.json

# Stop TigerBeetle
docker compose -f docker-compose-production.yml stop tigerbeetle

# Backup data volume
docker run --rm -v tigerbeetle-data:/data -v $(pwd)/backup:/backup \
  alpine tar czf /backup/tigerbeetle-migration-$(date +%Y%m%d).tar.gz /data

# Format new TigerBeetle (if required)
# WARNING: This destroys existing data
docker run --rm -v tigerbeetle-data:/data tigerbeetle/tigerbeetle:new-version \
  format --cluster=0 --replica=0 --replica-count=1 /data/0_0.tigerbeetle

# Import data (if migration script provided)
docker exec tigerbeetle tigerbeetle-import /data/export.json

# Start services
docker compose -f docker-compose-production.yml up -d
```

#### Configuration Migration

```bash
# Check for configuration changes
diff .env.example .env.example.new

# Apply new required variables
echo "NEW_VAR=value" >> .env

# Migrate YAML configuration if format changed
# Review release notes for specific instructions
```

#### Wallet Database Migration

```bash
# Stop connector
docker compose -f docker-compose-production.yml stop connector

# Backup wallet database
cp data/wallet/agent-wallets.db data/wallet/agent-wallets.db.backup

# Run migration script (if provided)
npx @crosstown/connector migrate-wallet --from=1.0 --to=2.0

# Verify migration
sqlite3 data/wallet/agent-wallets.db "SELECT count(*) FROM wallets;"
```

---

## Breaking Changes

### v2.0.0 Breaking Changes {#breaking-changes-v2}

_Note: This section will be updated when v2.0.0 is released._

**Configuration Changes:**

- `SETTLEMENT_METHOD` renamed to `SETTLEMENT_PREFERENCE`
- `BTP_PORT` renamed to `BTP_SERVER_PORT`

**API Changes:**

- `/health` response format extended with `dependencies` field
- `/settlement/execute` request body format changed

**Migration Steps:**

1. Update environment variables in `.env`
2. Review and update peer configuration YAML
3. Test in staging before production upgrade

### v1.2.0 Changes (Non-Breaking)

- New `OTEL_ENABLED` environment variable for tracing
- New `/metrics` endpoint for Prometheus
- Extended health check with SLA metrics

---

## Blue-Green Deployment

For zero-downtime upgrades, use blue-green deployment:

### Setup

```bash
# Assume current "blue" environment is running
# Create "green" environment with new version

# 1. Start green environment on different ports
docker compose -f docker-compose-green.yml up -d

# 2. Wait for green to be healthy
curl http://localhost:8081/health | jq .

# 3. Run smoke tests against green
./scripts/smoke-test.sh --host localhost --port 8081

# 4. Switch traffic (update load balancer/DNS)
# Point traffic to green (port 8081)

# 5. Verify production traffic
curl https://production.example.com/health | jq .

# 6. Stop blue environment
docker compose -f docker-compose-blue.yml down
```

### Rollback (Blue-Green)

```bash
# Simply switch traffic back to blue
# Update load balancer/DNS to point to blue

# Stop green
docker compose -f docker-compose-green.yml down
```

---

## Post-Upgrade Verification

### Immediate Checks (0-5 minutes)

```bash
# 1. Health check
curl http://localhost:8080/health | jq .

# 2. Verify version
docker inspect agent-runtime --format='{{.Config.Image}}'

# 3. Check logs for errors
docker logs agent-runtime --since 5m 2>&1 | grep -i error

# 4. Verify peer connections
curl http://localhost:8080/health | jq '.peers'

# 5. Check metrics endpoint
curl http://localhost:8080/metrics | head -20
```

### Short-Term Monitoring (5-60 minutes)

```bash
# Monitor error rate
watch -n 10 'curl -s http://localhost:8080/metrics | grep error_total'

# Monitor packet processing
watch -n 5 'curl -s http://localhost:8080/metrics | grep ilp_packets_processed'

# Monitor settlement activity
watch -n 30 'curl -s http://localhost:8080/metrics | grep settlement'
```

### Extended Monitoring (1-24 hours)

- [ ] Monitor Grafana dashboards for anomalies
- [ ] Review alert history for any triggered alerts
- [ ] Check settlement success rate (target: >99%)
- [ ] Verify packet processing latency (target: p99 <10ms)
- [ ] Confirm no memory leaks (stable heap usage)

---

## Troubleshooting

### Upgrade Issues

#### Issue: Container fails to start after upgrade

**Symptoms:**

- Container exits immediately
- Health check never passes

**Resolution:**

```bash
# Check container logs
docker logs agent-runtime --tail 200

# Common causes:
# 1. Missing environment variables
docker compose config | grep -i "variable"

# 2. Configuration file format error
docker compose -f docker-compose-production.yml config --quiet

# 3. Port conflict
netstat -tlnp | grep 8080
```

#### Issue: TigerBeetle connection failed after upgrade

**Symptoms:**

- Health check shows TigerBeetle as "down"
- Settlement operations failing

**Resolution:**

```bash
# Check TigerBeetle container
docker logs tigerbeetle --tail 100

# Verify network connectivity
docker exec agent-runtime nc -zv tigerbeetle 3000

# Restart TigerBeetle if needed
docker compose -f docker-compose-production.yml restart tigerbeetle
```

#### Issue: Peer connections dropped after upgrade

**Symptoms:**

- Fewer peers connected than before
- Packet routing errors

**Resolution:**

```bash
# Check peer configuration
cat examples/production-single-node.yaml

# Verify BTP connectivity
docker logs agent-runtime 2>&1 | grep -i btp

# Check firewall rules
sudo ufw status
```

### Rollback Issues

#### Issue: Rollback fails - previous image not available

**Resolution:**

```bash
# Check available images
docker images | grep agent-runtime

# Pull specific version if needed
docker pull ghcr.io/your-org/agent-runtime:v1.1.0

# Try rollback again
./scripts/rollback.sh v1.1.0
```

#### Issue: Data incompatibility after rollback

**Symptoms:**

- Database errors after rolling back from v2.0 to v1.x

**Resolution:**

```bash
# Restore from pre-upgrade backup
cd /opt/m2m
docker compose -f docker-compose-production.yml stop

# Restore TigerBeetle data
docker run --rm -v tigerbeetle-data:/data -v $(pwd)/backup:/backup \
  alpine sh -c "rm -rf /data/* && tar xzf /backup/tigerbeetle-pre-upgrade-*.tar.gz -C /"

# Restore wallet data
rm -rf data/wallet
cp -r data/wallet.backup-* data/wallet

# Start with old version
export IMAGE_TAG=v1.1.0
docker compose -f docker-compose-production.yml up -d
```

---

## Support Resources

- **Release Notes**: [CHANGELOG.md](../../CHANGELOG.md)
- **Production Guide**: [production-deployment-guide.md](./production-deployment-guide.md)
- **Incident Response**: [incident-response-runbook.md](./incident-response-runbook.md)
- **Monitoring**: [monitoring-setup-guide.md](./monitoring-setup-guide.md)
- **Issues**: https://github.com/m2m-network/m2m/issues

---

**Document Version**: 1.0
**Last Updated**: 2026-01-23
**Author**: Dev Agent James (Story 12.9)
