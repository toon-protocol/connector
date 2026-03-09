# Backup and Disaster Recovery Guide

This guide provides comprehensive backup and disaster recovery procedures for the M2M ILP Connector infrastructure, including TigerBeetle data, connector configuration, wallet state, and multi-region strategies.

## Table of Contents

1. [Overview](#overview)
2. [Recovery Objectives](#recovery-objectives)
3. [Backup Components](#backup-components)
4. [Backup Procedures](#backup-procedures)
5. [Disaster Recovery Procedures](#disaster-recovery-procedures)
6. [Multi-Region Backup Strategies](#multi-region-backup-strategies)
7. [Backup Scripts and Automation](#backup-scripts-and-automation)
8. [Testing and Validation](#testing-and-validation)
9. [Troubleshooting](#troubleshooting)

---

## Overview

The M2M ILP Connector requires backup of multiple components to ensure complete disaster recovery:

| Component        | Data Type               | Criticality | Backup Method   |
| ---------------- | ----------------------- | ----------- | --------------- |
| TigerBeetle      | Account balances        | Critical    | Volume snapshot |
| Connector Config | YAML/ENV files          | High        | File backup     |
| Wallet Data      | SQLite + encrypted seed | Critical    | Database export |
| Peer State       | Connection config       | Medium      | Config backup   |
| Monitoring State | Prometheus/Grafana      | Low         | Config backup   |

### Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                     Backup Architecture                          │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐       │
│  │  TigerBeetle │    │   Connector  │    │    Wallet    │       │
│  │    Data      │    │    Config    │    │    State     │       │
│  └──────┬───────┘    └──────┬───────┘    └──────┬───────┘       │
│         │                   │                   │                │
│         ▼                   ▼                   ▼                │
│  ┌─────────────────────────────────────────────────────────┐    │
│  │              Local Backup Storage                        │    │
│  │              /opt/m2m/backups/                           │    │
│  └──────────────────────────┬──────────────────────────────┘    │
│                             │                                    │
│         ┌───────────────────┼───────────────────┐               │
│         ▼                   ▼                   ▼               │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐       │
│  │   AWS S3     │    │   GCP GCS    │    │  Azure Blob  │       │
│  │  (Primary)   │    │  (Secondary) │    │  (Tertiary)  │       │
│  └──────────────┘    └──────────────┘    └──────────────┘       │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

---

## Recovery Objectives

### Recovery Time Objective (RTO)

| Scenario                 | Target RTO | Procedure             |
| ------------------------ | ---------- | --------------------- |
| Single component failure | 15 minutes | Container restart     |
| Full connector failure   | 1 hour     | Restore from backup   |
| Data center failure      | 4 hours    | Multi-region failover |
| Complete disaster        | 8 hours    | Full rebuild          |

### Recovery Point Objective (RPO)

| Component            | Target RPO             | Backup Frequency |
| -------------------- | ---------------------- | ---------------- |
| TigerBeetle balances | 1 hour                 | Hourly snapshots |
| Wallet state         | 24 hours               | Daily backups    |
| Configuration        | 0 (version controlled) | On change        |
| Monitoring data      | 7 days                 | Weekly export    |

### SLA Impact Assessment

| Outage Duration | Business Impact | Escalation Level |
| --------------- | --------------- | ---------------- |
| < 5 minutes     | Minimal         | L1 - On-call     |
| 5-30 minutes    | Low             | L1 - On-call     |
| 30-60 minutes   | Medium          | L2 - Team Lead   |
| 1-4 hours       | High            | L3 - Management  |
| > 4 hours       | Critical        | Executive        |

---

## Backup Components

### 1. TigerBeetle Data

TigerBeetle stores all account balances and transfer history.

**Location:** Docker volume `tigerbeetle-data`

**Critical Files:**

- `0_0.tigerbeetle` - Primary data file
- WAL (Write-Ahead Log) files

**Backup Method:** Docker volume snapshot

```bash
# Identify volume location
docker volume inspect tigerbeetle-data --format '{{.Mountpoint}}'
```

### 2. Connector Configuration

**Files to Backup:**

```
/opt/m2m/
├── .env                              # Environment configuration
├── examples/production-single-node.yaml  # Peer configuration
├── docker-compose-production.yml     # Docker stack definition
└── monitoring/                       # Monitoring configuration
    ├── prometheus/prometheus.yml
    ├── prometheus/alerts/*.yml
    ├── grafana/dashboards/*.json
    └── alertmanager/alertmanager.yml
```

### 3. Wallet Data

**Files to Backup:**

```
/opt/m2m/data/wallet/
├── master-seed.enc     # Encrypted master seed (CRITICAL)
├── agent-wallets.db    # SQLite wallet metadata
└── audit-log.db        # Audit trail
```

See [wallet-backup-recovery.md](./wallet-backup-recovery.md) for wallet-specific procedures.

### 4. Secrets and Credentials

**Never backup in plaintext:**

- KMS key references (safe to backup)
- API keys (store in secrets manager)
- TLS certificates (backup separately)

---

## Backup Procedures

### Daily Incremental Backup

```bash
#!/bin/bash
# daily-backup.sh - Run via cron at 2 AM

set -euo pipefail

BACKUP_DIR="/opt/m2m/backups"
DATE=$(date +%Y%m%d)
RETENTION_DAYS=30

echo "[$(date)] Starting daily backup..."

# 1. TigerBeetle hot backup (no downtime)
docker exec tigerbeetle tigerbeetle checkpoint /data/checkpoint-$DATE

# 2. Copy checkpoint to backup location
docker run --rm \
  -v tigerbeetle-data:/data:ro \
  -v $BACKUP_DIR:/backup \
  alpine tar czf /backup/tigerbeetle-$DATE.tar.gz /data/checkpoint-$DATE

# 3. Backup configuration files
tar czf $BACKUP_DIR/config-$DATE.tar.gz \
  /opt/m2m/.env \
  /opt/m2m/examples/*.yaml \
  /opt/m2m/docker-compose-production.yml

# 4. Backup wallet data
tar czf $BACKUP_DIR/wallet-$DATE.tar.gz \
  /opt/m2m/data/wallet/

# 5. Create manifest
cat > $BACKUP_DIR/manifest-$DATE.json << EOF
{
  "date": "$DATE",
  "type": "daily",
  "files": [
    "tigerbeetle-$DATE.tar.gz",
    "config-$DATE.tar.gz",
    "wallet-$DATE.tar.gz"
  ],
  "checksum": "$(sha256sum $BACKUP_DIR/tigerbeetle-$DATE.tar.gz | cut -d' ' -f1)"
}
EOF

# 6. Upload to cloud storage
aws s3 cp $BACKUP_DIR/tigerbeetle-$DATE.tar.gz s3://m2m-backups/daily/
aws s3 cp $BACKUP_DIR/config-$DATE.tar.gz s3://m2m-backups/daily/
aws s3 cp $BACKUP_DIR/wallet-$DATE.tar.gz s3://m2m-backups/daily/
aws s3 cp $BACKUP_DIR/manifest-$DATE.json s3://m2m-backups/daily/

# 7. Clean up old local backups
find $BACKUP_DIR -name "*.tar.gz" -mtime +$RETENTION_DAYS -delete
find $BACKUP_DIR -name "manifest-*.json" -mtime +$RETENTION_DAYS -delete

echo "[$(date)] Daily backup completed successfully"
```

### Weekly Full Backup

```bash
#!/bin/bash
# weekly-backup.sh - Run via cron Sunday 3 AM

set -euo pipefail

BACKUP_DIR="/opt/m2m/backups/weekly"
DATE=$(date +%Y%m%d)
RETENTION_WEEKS=12

echo "[$(date)] Starting weekly full backup..."

# Stop connector for consistent backup
docker compose -f /opt/m2m/docker-compose-production.yml stop connector

# Full TigerBeetle backup
docker run --rm \
  -v tigerbeetle-data:/data:ro \
  -v $BACKUP_DIR:/backup \
  alpine tar czf /backup/tigerbeetle-full-$DATE.tar.gz /data

# Full configuration backup (including secrets references)
tar czf $BACKUP_DIR/full-config-$DATE.tar.gz \
  /opt/m2m/.env \
  /opt/m2m/examples/ \
  /opt/m2m/docker-compose*.yml \
  /opt/m2m/monitoring/

# Full wallet backup
tar czf $BACKUP_DIR/full-wallet-$DATE.tar.gz \
  /opt/m2m/data/

# Restart connector
docker compose -f /opt/m2m/docker-compose-production.yml start connector

# Create checksum file
sha256sum $BACKUP_DIR/*-$DATE.tar.gz > $BACKUP_DIR/checksums-$DATE.txt

# Upload to multiple cloud providers
aws s3 cp $BACKUP_DIR/ s3://m2m-backups/weekly/$DATE/ --recursive
gsutil -m cp $BACKUP_DIR/*.tar.gz gs://m2m-backups-secondary/weekly/$DATE/

echo "[$(date)] Weekly full backup completed"
```

### Cron Schedule Configuration

```cron
# /etc/cron.d/m2m-backup

# Daily incremental backup at 2 AM
0 2 * * * root /opt/m2m/scripts/daily-backup.sh >> /var/log/m2m-backup.log 2>&1

# Weekly full backup at 3 AM Sunday
0 3 * * 0 root /opt/m2m/scripts/weekly-backup.sh >> /var/log/m2m-backup.log 2>&1

# Monthly off-site archive at 4 AM 1st of month
0 4 1 * * root /opt/m2m/scripts/monthly-archive.sh >> /var/log/m2m-backup.log 2>&1
```

---

## Disaster Recovery Procedures

### Scenario 1: Single Container Failure

**Symptoms:** One service (connector, TigerBeetle) is down but data is intact.

**Recovery Time:** ~5 minutes

```bash
# 1. Identify failed container
docker compose -f docker-compose-production.yml ps

# 2. Check logs
docker logs agent-runtime --tail 100

# 3. Restart failed container
docker compose -f docker-compose-production.yml restart connector

# 4. Verify health
curl http://localhost:8080/health | jq .
```

### Scenario 2: Data Corruption

**Symptoms:** TigerBeetle reporting data errors, balance inconsistencies.

**Recovery Time:** ~30-60 minutes

```bash
# 1. Stop all services
docker compose -f docker-compose-production.yml down

# 2. Identify latest good backup
ls -la /opt/m2m/backups/ | tail -10
cat /opt/m2m/backups/manifest-*.json | tail -1

# 3. Remove corrupted data
docker volume rm tigerbeetle-data

# 4. Recreate volume
docker volume create tigerbeetle-data

# 5. Restore from backup
docker run --rm \
  -v tigerbeetle-data:/data \
  -v /opt/m2m/backups:/backup:ro \
  alpine tar xzf /backup/tigerbeetle-YYYYMMDD.tar.gz -C /

# 6. Restart services
docker compose -f docker-compose-production.yml up -d

# 7. Verify data integrity
curl http://localhost:8080/health | jq '.dependencies.tigerbeetle'
```

### Scenario 3: Complete Server Failure

**Symptoms:** Server is unrecoverable, need to rebuild on new hardware.

**Recovery Time:** ~2-4 hours

```bash
# On new server:

# 1. Install prerequisites
curl -fsSL https://get.docker.com | sh
sudo usermod -aG docker $USER

# 2. Clone repository
git clone https://github.com/m2m-network/m2m.git /opt/m2m
cd /opt/m2m

# 3. Download latest backup from cloud
aws s3 cp s3://m2m-backups/weekly/latest/ /opt/m2m/restore/ --recursive

# 4. Verify backup checksums
sha256sum -c /opt/m2m/restore/checksums-*.txt

# 5. Restore TigerBeetle data
docker volume create tigerbeetle-data
docker run --rm \
  -v tigerbeetle-data:/data \
  -v /opt/m2m/restore:/backup:ro \
  alpine tar xzf /backup/tigerbeetle-full-*.tar.gz -C /

# 6. Restore configuration
tar xzf /opt/m2m/restore/full-config-*.tar.gz -C /

# 7. Restore wallet data
tar xzf /opt/m2m/restore/full-wallet-*.tar.gz -C /

# 8. Update secrets (get from secrets manager)
# AWS KMS keys, API tokens, etc.

# 9. Start services
docker compose -f docker-compose-production.yml up -d

# 10. Verify health
curl http://localhost:8080/health | jq .

# 11. Reconcile balances (check for transactions during downtime)
# Review on-chain state vs backup state
```

### Scenario 4: Data Center Failure

**Symptoms:** Entire data center is unavailable.

**Recovery Time:** ~4-8 hours

See [Multi-Region Backup Strategies](#multi-region-backup-strategies) for failover procedures.

---

## Multi-Region Backup Strategies

### Primary-Secondary Architecture

```
┌─────────────────────┐         ┌─────────────────────┐
│   Primary Region    │         │  Secondary Region   │
│    (US-East-1)      │         │    (EU-West-1)      │
├─────────────────────┤         ├─────────────────────┤
│                     │         │                     │
│  ┌─────────────┐    │         │  ┌─────────────┐    │
│  │  Connector  │    │         │  │  Standby    │    │
│  │  (Active)   │    │  Sync   │  │  Connector  │    │
│  └──────┬──────┘    │ ──────► │  └──────┬──────┘    │
│         │           │         │         │           │
│  ┌──────▼──────┐    │         │  ┌──────▼──────┐    │
│  │ TigerBeetle │    │         │  │ TigerBeetle │    │
│  │  (Primary)  │    │ ──────► │  │  (Replica)  │    │
│  └─────────────┘    │         │  └─────────────┘    │
│                     │         │                     │
└─────────────────────┘         └─────────────────────┘
```

### Cross-Region Backup Replication

```bash
# S3 Cross-Region Replication (configured in AWS)
aws s3api put-bucket-replication \
  --bucket m2m-backups-primary \
  --replication-configuration file://replication-config.json
```

**replication-config.json:**

```json
{
  "Role": "arn:aws:iam::123456789012:role/replication-role",
  "Rules": [
    {
      "Status": "Enabled",
      "Priority": 1,
      "Filter": {},
      "Destination": {
        "Bucket": "arn:aws:s3:::m2m-backups-secondary",
        "StorageClass": "STANDARD_IA"
      },
      "DeleteMarkerReplication": {
        "Status": "Disabled"
      }
    }
  ]
}
```

### Failover Procedure

```bash
#!/bin/bash
# failover-to-secondary.sh

set -euo pipefail

SECONDARY_HOST="secondary.m2m.example.com"

echo "[$(date)] Initiating failover to secondary region..."

# 1. Verify primary is down
if curl -sf https://primary.m2m.example.com/health > /dev/null 2>&1; then
  echo "Primary is still responding. Aborting failover."
  exit 1
fi

# 2. Get latest backup from secondary region's S3
ssh $SECONDARY_HOST << 'EOF'
  cd /opt/m2m
  aws s3 sync s3://m2m-backups-secondary/daily/latest/ /opt/m2m/restore/
EOF

# 3. Restore and start secondary
ssh $SECONDARY_HOST << 'EOF'
  cd /opt/m2m
  ./scripts/restore-from-backup.sh /opt/m2m/restore/
  docker compose -f docker-compose-production.yml up -d
EOF

# 4. Update DNS to point to secondary
aws route53 change-resource-record-sets \
  --hosted-zone-id Z123456 \
  --change-batch file://dns-failover.json

# 5. Verify secondary is serving traffic
sleep 30
curl https://m2m.example.com/health | jq .

echo "[$(date)] Failover complete. Secondary region is now active."
```

---

## Backup Scripts and Automation

### Complete Backup Script

```bash
#!/bin/bash
# /opt/m2m/scripts/backup.sh
#
# Usage: ./backup.sh [--full|--incremental] [--verify] [--upload]

set -euo pipefail

BACKUP_TYPE="${1:---incremental}"
VERIFY="${2:-}"
UPLOAD="${3:-}"

BACKUP_DIR="/opt/m2m/backups"
DATE=$(date +%Y%m%d-%H%M%S)
MANIFEST_FILE="$BACKUP_DIR/manifest-$DATE.json"

log() { echo "[$(date '+%Y-%m-%d %H:%M:%S')] $1"; }

log "Starting $BACKUP_TYPE backup..."

mkdir -p $BACKUP_DIR

# Create backup based on type
if [ "$BACKUP_TYPE" == "--full" ]; then
  # Stop for consistent full backup
  docker compose -f /opt/m2m/docker-compose-production.yml stop connector
  DOWNTIME_START=$(date +%s)

  # Full TigerBeetle backup
  docker run --rm \
    -v tigerbeetle-data:/data:ro \
    -v $BACKUP_DIR:/backup \
    alpine tar czf /backup/tigerbeetle-full-$DATE.tar.gz /data

  # Full config and wallet
  tar czf $BACKUP_DIR/config-full-$DATE.tar.gz \
    /opt/m2m/.env \
    /opt/m2m/examples/ \
    /opt/m2m/docker-compose*.yml \
    /opt/m2m/monitoring/ 2>/dev/null || true

  tar czf $BACKUP_DIR/wallet-full-$DATE.tar.gz \
    /opt/m2m/data/ 2>/dev/null || true

  # Restart connector
  docker compose -f /opt/m2m/docker-compose-production.yml start connector
  DOWNTIME_END=$(date +%s)
  DOWNTIME=$((DOWNTIME_END - DOWNTIME_START))
  log "Downtime: ${DOWNTIME} seconds"
else
  # Incremental backup (no downtime)
  docker exec tigerbeetle tigerbeetle checkpoint /data/checkpoint-$DATE 2>/dev/null || true

  docker run --rm \
    -v tigerbeetle-data:/data:ro \
    -v $BACKUP_DIR:/backup \
    alpine tar czf /backup/tigerbeetle-incr-$DATE.tar.gz /data/checkpoint-$DATE 2>/dev/null || \
    docker run --rm \
      -v tigerbeetle-data:/data:ro \
      -v $BACKUP_DIR:/backup \
      alpine tar czf /backup/tigerbeetle-incr-$DATE.tar.gz /data

  tar czf $BACKUP_DIR/config-incr-$DATE.tar.gz \
    /opt/m2m/.env \
    /opt/m2m/examples/*.yaml 2>/dev/null || true

  DOWNTIME=0
fi

# Generate checksums
CHECKSUMS=""
for f in $BACKUP_DIR/*-$DATE.tar.gz; do
  if [ -f "$f" ]; then
    CHECKSUM=$(sha256sum "$f" | cut -d' ' -f1)
    FILENAME=$(basename "$f")
    CHECKSUMS="$CHECKSUMS\"$FILENAME\": \"$CHECKSUM\","
  fi
done
CHECKSUMS="${CHECKSUMS%,}"  # Remove trailing comma

# Create manifest
cat > $MANIFEST_FILE << EOF
{
  "date": "$DATE",
  "type": "${BACKUP_TYPE#--}",
  "downtime_seconds": $DOWNTIME,
  "files": {
    $CHECKSUMS
  },
  "verified": false
}
EOF

log "Manifest created: $MANIFEST_FILE"

# Verify backup integrity
if [ "$VERIFY" == "--verify" ]; then
  log "Verifying backup integrity..."
  VERIFY_FAILED=0

  for f in $BACKUP_DIR/*-$DATE.tar.gz; do
    if [ -f "$f" ]; then
      if tar -tzf "$f" > /dev/null 2>&1; then
        log "✓ $(basename $f) - OK"
      else
        log "✗ $(basename $f) - CORRUPTED"
        VERIFY_FAILED=1
      fi
    fi
  done

  if [ $VERIFY_FAILED -eq 0 ]; then
    # Update manifest
    sed -i 's/"verified": false/"verified": true/' $MANIFEST_FILE
    log "All backups verified successfully"
  else
    log "ERROR: Some backups failed verification"
    exit 1
  fi
fi

# Upload to cloud storage
if [ "$UPLOAD" == "--upload" ]; then
  log "Uploading to cloud storage..."

  # AWS S3
  aws s3 cp $BACKUP_DIR/ s3://m2m-backups/$(date +%Y/%m/%d)/ \
    --recursive --exclude "*" --include "*-$DATE.*"

  log "Upload complete"
fi

log "Backup completed successfully"
echo "$MANIFEST_FILE"
```

### Restore Script

```bash
#!/bin/bash
# /opt/m2m/scripts/restore.sh
#
# Usage: ./restore.sh <backup-path> [--verify-only]

set -euo pipefail

BACKUP_PATH="${1:-}"
VERIFY_ONLY="${2:-}"

if [ -z "$BACKUP_PATH" ]; then
  echo "Usage: ./restore.sh <backup-path> [--verify-only]"
  exit 1
fi

log() { echo "[$(date '+%Y-%m-%d %H:%M:%S')] $1"; }

log "Starting restore from $BACKUP_PATH..."

# Find backup files
TB_BACKUP=$(ls $BACKUP_PATH/tigerbeetle-*.tar.gz 2>/dev/null | head -1)
CONFIG_BACKUP=$(ls $BACKUP_PATH/config-*.tar.gz 2>/dev/null | head -1)
WALLET_BACKUP=$(ls $BACKUP_PATH/wallet-*.tar.gz 2>/dev/null | head -1)
MANIFEST=$(ls $BACKUP_PATH/manifest-*.json 2>/dev/null | head -1)

if [ -z "$TB_BACKUP" ]; then
  log "ERROR: No TigerBeetle backup found"
  exit 1
fi

# Verify checksums if manifest exists
if [ -n "$MANIFEST" ]; then
  log "Verifying backup checksums..."
  # Parse manifest and verify (simplified)
  for f in $TB_BACKUP $CONFIG_BACKUP $WALLET_BACKUP; do
    if [ -f "$f" ]; then
      tar -tzf "$f" > /dev/null 2>&1 || {
        log "ERROR: $f failed integrity check"
        exit 1
      }
      log "✓ $(basename $f) verified"
    fi
  done
fi

if [ "$VERIFY_ONLY" == "--verify-only" ]; then
  log "Verification complete (--verify-only mode)"
  exit 0
fi

# Stop services
log "Stopping services..."
docker compose -f /opt/m2m/docker-compose-production.yml down

# Restore TigerBeetle
log "Restoring TigerBeetle data..."
docker volume rm tigerbeetle-data 2>/dev/null || true
docker volume create tigerbeetle-data
docker run --rm \
  -v tigerbeetle-data:/data \
  -v $(dirname $TB_BACKUP):/backup:ro \
  alpine tar xzf /backup/$(basename $TB_BACKUP) -C /

# Restore configuration
if [ -n "$CONFIG_BACKUP" ]; then
  log "Restoring configuration..."
  tar xzf $CONFIG_BACKUP -C /
fi

# Restore wallet
if [ -n "$WALLET_BACKUP" ]; then
  log "Restoring wallet data..."
  tar xzf $WALLET_BACKUP -C /
fi

# Start services
log "Starting services..."
docker compose -f /opt/m2m/docker-compose-production.yml up -d

# Wait for health
log "Waiting for health check..."
for i in {1..24}; do
  if curl -sf http://localhost:8080/health > /dev/null 2>&1; then
    log "✓ Health check passed!"
    exit 0
  fi
  log "Attempt $i/24: Waiting..."
  sleep 5
done

log "ERROR: Health check failed after restore"
exit 1
```

---

## Testing and Validation

### Quarterly DR Drill Checklist

- [ ] **Schedule maintenance window** (4 hours minimum)
- [ ] **Notify stakeholders**
- [ ] **Create fresh backup before drill**
- [ ] **Provision test environment** (isolated from production)
- [ ] **Execute restore procedure** (timed)
- [ ] **Verify all services healthy**
- [ ] **Run smoke tests**
- [ ] **Verify data integrity**
- [ ] **Document results and issues**
- [ ] **Update procedures if needed**

### Automated Backup Verification

```bash
#!/bin/bash
# weekly-backup-verify.sh - Run weekly to verify backup integrity

set -euo pipefail

BACKUP_DIR="/opt/m2m/backups"
ALERT_EMAIL="ops@example.com"

log() { echo "[$(date '+%Y-%m-%d %H:%M:%S')] $1"; }

# Get latest backup
LATEST_BACKUP=$(ls -t $BACKUP_DIR/manifest-*.json | head -1)

if [ -z "$LATEST_BACKUP" ]; then
  echo "ALERT: No backup manifest found" | mail -s "Backup Verification FAILED" $ALERT_EMAIL
  exit 1
fi

BACKUP_DATE=$(cat $LATEST_BACKUP | jq -r '.date')
BACKUP_AGE=$(( ($(date +%s) - $(date -d "${BACKUP_DATE:0:8}" +%s)) / 86400 ))

# Check backup age
if [ $BACKUP_AGE -gt 2 ]; then
  echo "ALERT: Latest backup is $BACKUP_AGE days old" | mail -s "Backup Age WARNING" $ALERT_EMAIL
fi

# Verify backup files exist and are readable
VERIFIED=$(cat $LATEST_BACKUP | jq -r '.verified')
if [ "$VERIFIED" != "true" ]; then
  log "Running verification..."
  /opt/m2m/scripts/backup.sh --verify
fi

log "Backup verification complete. Latest backup: $BACKUP_DATE ($BACKUP_AGE days old)"
```

---

## Troubleshooting

### Common Issues

| Issue                              | Cause                               | Solution                                                   |
| ---------------------------------- | ----------------------------------- | ---------------------------------------------------------- |
| Backup checksum mismatch           | Disk corruption or incomplete write | Retry backup, check disk health                            |
| Restore fails with "volume in use" | Container still running             | Stop all containers before restore                         |
| Cloud upload timeout               | Large backup or slow connection     | Use multipart upload, increase timeout                     |
| Insufficient disk space            | Backups accumulating                | Review retention policy, clean old backups                 |
| Permission denied                  | Wrong user/group ownership          | Fix permissions: `chown -R deploy:deploy /opt/m2m/backups` |

### Diagnostic Commands

```bash
# Check backup directory size
du -sh /opt/m2m/backups/

# List recent backups
ls -lah /opt/m2m/backups/*.tar.gz | tail -10

# Verify tarball integrity
tar -tzf /opt/m2m/backups/tigerbeetle-*.tar.gz | head

# Check backup cron job status
systemctl status cron
grep m2m-backup /var/log/syslog | tail -20

# Verify S3 sync status
aws s3 ls s3://m2m-backups/daily/ --recursive | tail -10
```

---

## Support Resources

- **Wallet Backup Guide**: [wallet-backup-recovery.md](./wallet-backup-recovery.md)
- **Upgrade Guide**: [upgrade-migration-guide.md](./upgrade-migration-guide.md)
- **Incident Response**: [incident-response-runbook.md](./incident-response-runbook.md)
- **Production Guide**: [production-deployment-guide.md](./production-deployment-guide.md)

---

**Document Version**: 1.0
**Last Updated**: 2026-01-23
**Author**: Dev Agent James (Story 12.9)
