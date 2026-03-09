# Wallet Backup and Recovery Procedures

## Overview

The M2M AI Agent Wallet Infrastructure includes comprehensive backup and recovery capabilities to protect agent wallet data from catastrophic failures. This document provides step-by-step procedures for backing up and restoring agent wallet state.

## Backup Types

The system supports two types of backups:

### Full Backup

- **Frequency**: Weekly (default: Sunday midnight)
- **Contents**: All agent wallets, master seed, lifecycle records, balance snapshots
- **Use Case**: Complete system recovery, long-term archival
- **File Size**: ~1KB per wallet (~10MB for 10,000 wallets)

### Incremental Backup

- **Frequency**: Daily (default: midnight)
- **Contents**: Only wallets and records modified since last backup, plus master seed
- **Use Case**: Daily protection with minimal storage overhead
- **File Size**: Typically <1% of full backup size

## Backup Security

### Encryption

- Master seed always encrypted with AES-256-GCM
- Password-based key derivation using PBKDF2 (100,000 iterations)
- Strong password required (16+ chars, uppercase, lowercase, number, symbol)

### Integrity Validation

- SHA-256 checksum over entire backup file
- Checksum verified before restore to prevent corruption
- Backup rejected if checksum validation fails

### Storage Locations

- **Local Filesystem**: Default `./backups/wallet-backup-{timestamp}.json`
- **Amazon S3**: Optional cloud backup for redundancy
- **Permissions**: Local files should be restricted to owner only (`chmod 600`)

## Automated Backup Schedule

Backups run automatically on a cron schedule:

- **Full Backup**: `0 0 * * 0` (Sunday midnight)
- **Incremental Backup**: `0 0 * * *` (Daily midnight)

Configuration via `BackupConfig`:

```typescript
const config: BackupConfig = {
  backupPath: './backups',
  s3Bucket: 'my-wallet-backups', // Optional
  s3Region: 'us-east-1',
  s3AccessKeyId: process.env.AWS_ACCESS_KEY_ID,
  s3SecretAccessKey: process.env.AWS_SECRET_ACCESS_KEY,
  backupPassword: process.env.WALLET_BACKUP_PASSWORD,
  fullBackupSchedule: '0 0 * * 0', // Weekly
  incrementalBackupSchedule: '0 0 * * *', // Daily
};
```

## Manual Backup Creation

To trigger an ad-hoc backup outside the scheduled times:

```typescript
// Full backup
const backup = await backupManager.createFullBackup('your-strong-password');
console.log(`Full backup created: ${backup.timestamp}`);

// Incremental backup
const incrementalBackup = await backupManager.createIncrementalBackup('your-strong-password');
console.log(`Incremental backup created: ${incrementalBackup.timestamp}`);
```

## Disaster Recovery Procedure

Follow these steps to restore agent wallet state from a backup file:

### Step 1: Stop Connector Service

Prevent conflicting writes during restore:

```bash
docker-compose down
# OR
systemctl stop agent-runtime
```

### Step 2: Locate Backup File

Find the most recent full backup and any subsequent incremental backups:

```bash
ls -lt ./backups/wallet-backup-*.json | head -5
```

For recovery, you'll need:

- The most recent full backup
- All incremental backups created after that full backup (apply in chronological order)

### Step 3: Load Backup

```typescript
const backup = await backupManager.loadBackupFromFile('./backups/wallet-backup-1234567890.json');
console.log(`Loaded backup from ${new Date(backup.timestamp).toISOString()}`);
console.log(`Backup type: ${backup.type}`);
console.log(`Wallet count: ${backup.wallets.length}`);
```

### Step 4: Restore from Backup

```typescript
await backupManager.restoreFromBackup(backup, 'your-strong-password');
console.log('Wallet restore completed successfully');
```

The restore process:

1. ✅ Validates backup checksum
2. ✅ Decrypts and imports master seed
3. ✅ Restores wallet metadata for all agents
4. ✅ Restores lifecycle records (states, activity history)
5. ✅ Reconciles on-chain balances with backup snapshots

### Step 5: Verify Balance Reconciliation

Check logs for balance mismatches:

```bash
grep "Balance mismatch detected" ./logs/connector.log
```

Balance mismatches indicate:

- Transactions occurred during downtime
- On-chain state diverged from backup
- Manual investigation may be required

### Step 6: Restart Connector Service

Once restore completes and balance reconciliation is verified:

```bash
docker-compose up -d
# OR
systemctl start agent-runtime
```

## Backup Storage Best Practices

### Redundancy

- **Local + S3**: Configure both local filesystem and S3 for redundancy
- **Off-Site**: Store backups in geographically separate location
- **Cross-Region**: Use S3 cross-region replication for disaster recovery

### Retention Policy

- **Full Backups**: Retain for 1 year (52 weekly backups)
- **Incremental Backups**: Retain for 30 days
- **Automated Cleanup**: Implement rotation script to delete old backups

Example cleanup script:

```bash
#!/bin/bash
# Delete backups older than 30 days
find ./backups/wallet-backup-*.json -mtime +30 -delete
```

### Access Control

- **Filesystem Permissions**: `chmod 600 ./backups/*.json` (owner read/write only)
- **S3 Bucket Policy**: Restrict access to specific IAM role
- **Encryption at Rest**: Enable S3 SSE-S3 or SSE-KMS

### Password Management

- **Secure Storage**: Store backup password in password manager, HSM, or KMS
- **Rotation**: Rotate backup password quarterly
- **Backup Password**: Never commit password to version control

## Testing Backup and Recovery

### Quarterly Disaster Recovery Drills

Test backup and recovery procedures every quarter:

1. **Create Test Backup**: Trigger manual full backup
2. **Simulate Disaster**: Destroy test database (NOT production!)
3. **Restore from Backup**: Follow disaster recovery procedure
4. **Verify Restoration**: Confirm all wallets and balances restored
5. **Document Results**: Record drill results and any issues

### Backup Validation

Periodically validate backup file integrity:

```typescript
const backup = await backupManager.loadBackupFromFile('./backups/wallet-backup-1234567890.json');
const isValid = backupManager['validateBackup'](backup); // Access private method for validation
console.log(`Backup valid: ${isValid}`);
```

## Troubleshooting

### Issue: "Backup checksum validation failed"

**Cause**: Backup file corrupted or tampered with

**Solution**:

1. Try loading an earlier backup file
2. If S3 enabled, download backup from S3
3. Contact support if all backups corrupted

### Issue: "Invalid password or corrupted data"

**Cause**: Incorrect backup password or corrupted master seed

**Solution**:

1. Verify backup password (check password manager)
2. Try earlier backup file
3. If password lost, recovery impossible (master seed encrypted)

### Issue: "Balance mismatch detected" (multiple wallets)

**Cause**: Transactions occurred during downtime between backup and restore

**Solution**:

1. Review logs for each mismatched wallet
2. Query blockchain directly to verify on-chain balance
3. Manual reconciliation may be required
4. Update balance tracker with correct balances

### Issue: "Failed to upload backup to S3"

**Cause**: S3 credentials invalid, network issue, or bucket permissions

**Solution**:

1. Verify S3 credentials in config
2. Check IAM role has `s3:PutObject` permission
3. Test network connectivity to S3
4. **Important**: Local backup still succeeded, S3 failure is non-blocking

### Issue: Backup file size too large (>100MB)

**Cause**: Large number of wallets (>100,000) or balance history

**Solution**:

1. Use incremental backups more frequently
2. Compress backups: `gzip ./backups/wallet-backup-*.json`
3. Implement backup sharding for very large deployments
4. Consider archiving old balance snapshots

## Architecture Integration

The `WalletBackupManager` integrates with:

- **Story 11.1 (WalletSeedManager)**: Master seed export/import
- **Story 11.2 (AgentWalletDerivation)**: Wallet metadata export/import
- **Story 11.3 (AgentBalanceTracker)**: Balance snapshot export and reconciliation
- **Story 11.5 (AgentWalletLifecycle)**: Lifecycle record export/import

All backup operations are logged via Pino logger with structured JSON output.

## Support

For backup and recovery issues:

1. Check logs: `./logs/connector.log`
2. Verify backup file integrity (checksum validation)
3. Review this documentation
4. Contact platform support with backup timestamp and error logs

---

**Last Updated**: 2026-01-21
**Version**: 1.0 (Story 11.8)
