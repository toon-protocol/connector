# Agent Wallet Production Readiness Checklist

Complete this checklist before deploying agent wallet infrastructure to production.

## Overview

This checklist ensures all security, infrastructure, monitoring, and operational requirements are met for production deployment of agent wallet infrastructure.

**Target Audience:** DevOps, SRE, Security teams

**Completion Criteria:** All ✅ items must be checked before production deployment.

---

## Infrastructure Checklist

### Master Seed Management

- [ ] **Master seed generated in secure environment**
  - Generated on air-gapped machine or HSM
  - Not generated on production server
  - Seed generation audited and documented

- [ ] **Master seed encrypted with AES-256-GCM**
  - Password meets complexity requirements (16+ characters)
  - Encryption verified with `crypto.getCipherInfo()`
  - PBKDF2 iterations set to 100,000+

- [ ] **Master seed backed up to secure location**
  - Encrypted backup created immediately after generation
  - Backup stored in geographically distributed locations (minimum 2)
  - Backup NOT in version control

- [ ] **Master seed password stored securely**
  - Password stored in enterprise password manager (1Password, LastPass, Vault)
  - Password NOT in code, config files, or environment variables
  - Password access restricted to authorized personnel only

- [ ] **Master seed file permissions set correctly**

  ```bash
  chmod 600 data/wallet/master-seed.enc
  chown connector:connector data/wallet/master-seed.enc
  ```

  - Permissions: `-rw-------` (600)
  - Owner: Application user only

**Verification:**

```bash
# Check file permissions
ls -la data/wallet/master-seed.enc
# Should show: -rw------- 1 connector connector ... master-seed.enc

# Verify encryption
file data/wallet/master-seed.enc
# Should NOT show: ASCII text or JSON
```

---

### Database Configuration

- [ ] **TigerBeetle deployed for balance tracking**
  - TigerBeetle cluster deployed (3+ nodes for HA)
  - Persistent storage configured (SSD recommended)
  - Backup strategy defined (snapshot + WAL archival)
  - Connection pooling configured

- [ ] **SQLite configured for wallet metadata**
  - Database location: `data/wallet/agent-wallets.db`
  - Write-Ahead Logging (WAL) enabled
  - Database backed up regularly
  - File permissions: `600` (read/write owner only)

- [ ] **Audit log database configured**
  - Database location: `data/wallet/audit-log.db`
  - Append-only mode enabled
  - Retention policy defined (minimum 1 year)
  - Backup strategy configured

**Verification:**

```bash
# Check TigerBeetle status
tigerbeetle-cli status

# Verify SQLite WAL mode
sqlite3 data/wallet/agent-wallets.db "PRAGMA journal_mode;"
# Should output: wal

# Check database file permissions
ls -la data/wallet/*.db
```

---

### Blockchain RPC Endpoints

- [ ] **EVM (Base L2) RPC endpoint configured**
  - Commercial provider configured (Infura, Alchemy)
  - Fallback endpoints configured (minimum 2)
  - Rate limits appropriate for load
  - API key stored securely (secrets manager)

- [ ] **RPC endpoint failover tested**
  - Failover mechanism tested in staging
  - Automatic failover confirmed working
  - Failover alerts configured

**Configuration:**

```bash
# .env.production
EVM_RPC_ENDPOINT=https://base-mainnet.infura.io/v3/YOUR-API-KEY
EVM_RPC_FALLBACK=https://base.llamarpc.com,https://mainnet.base.org
```

**Verification:**

```bash
# Test EVM RPC
curl -X POST $EVM_RPC_ENDPOINT \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}'

```

---

### Backup Strategy

- [ ] **Automated backup schedule configured**
  - Daily incremental backups (metadata)
  - Weekly full backups (master seed + metadata)
  - Monthly off-site backups (offline storage)
  - Cron jobs configured and tested

- [ ] **Backup storage redundancy**
  - Local backups: `/secure/backups/` (encrypted filesystem)
  - Cloud backups: S3/GCS with versioning enabled
  - Offline backups: Secure physical location
  - Backup rotation policy defined (retain last 12 weeks)

- [ ] **Backup encryption verified**
  - All backups encrypted with strong password
  - Password different from master seed password
  - Backup password stored in secrets manager

**Backup Cron Schedule:**

```cron
# Daily incremental backup (2 AM)
0 2 * * * /usr/local/bin/wallet-backup daily >> /var/log/wallet-backup.log 2>&1

# Weekly full backup (3 AM Sunday)
0 3 * * 0 /usr/local/bin/wallet-backup weekly >> /var/log/wallet-backup.log 2>&1

# Monthly off-site backup (4 AM 1st of month)
0 4 1 * * /usr/local/bin/wallet-backup monthly >> /var/log/wallet-backup.log 2>&1
```

**Verification:**

```bash
# Test backup creation
/usr/local/bin/wallet-backup test

# Verify backup encryption
file /secure/backups/backup-latest.enc
# Should NOT show plaintext

# List recent backups
ls -lh /secure/backups/ | tail -20
```

---

## Security Checklist

### Authentication and Access Control

- [ ] **Password authentication configured**
  - Minimum password length: 16 characters (enforced)
  - Password complexity verified
  - Password stored as PBKDF2 hash (100k iterations)
  - Timing-safe comparison used

- [ ] **2FA enabled for production** (Epic 12)
  - TOTP configured with `speakeasy` library
  - QR code generation for setup
  - Backup codes generated and stored
  - 2FA required for sensitive operations

- [ ] **HSM integration configured** (Epic 12)
  - KeyManager configured with HSM credentials
  - HSM connectivity tested
  - Private keys stored in HSM
  - HSM backup/DR plan documented

- [ ] **Role-based access control (RBAC) implemented**
  - Roles defined: viewer, operator, admin, auditor
  - Permissions assigned per role
  - Principle of least privilege enforced
  - Access reviews scheduled (quarterly)

**Verification:**

```typescript
// Test authentication
import { AgentWalletAuthentication } from '@crosstown/connector/wallet/wallet-authentication';

const auth = new AgentWalletAuthentication();

await auth.authenticate({
  method: 'password',
  password: 'test-password-123',
});
// Should throw error if password incorrect
```

---

### Rate Limiting

- [ ] **Wallet creation rate limit configured**
  - Default: 100 wallets/hour
  - Adjusted based on expected legitimate load
  - Rate limit violations logged
  - Alerts configured for violations

- [ ] **Funding rate limit configured**
  - Separate limit from wallet creation (50/hour default)
  - Sliding window implementation
  - Burst protection enabled

- [ ] **Payment rate limit configured**
  - Default: 1000 payments/minute
  - Adjusted for expected payment volume
  - Per-agent and global limits

**Configuration:**

```bash
# Environment variables
WALLET_CREATION_RATE_LIMIT=100
WALLET_CREATION_RATE_WINDOW=3600  # seconds
FUNDING_RATE_LIMIT=50
PAYMENT_RATE_LIMIT=1000
```

**Verification:**

```bash
# Test rate limiting
for i in {1..101}; do
  curl -X POST http://localhost:3000/api/wallets \
    -H "Content-Type: application/json" \
    -d "{\"agentId\":\"test-agent-$i\"}"
done
# 101st request should fail with rate limit error
```

---

### Spending Limits

- [ ] **Default spending limits configured**
  - Max transaction size: 1000 USDC (configurable)
  - Daily spending limit: 5000 USDC
  - Monthly spending limit: 50,000 USDC
  - Limits documented in `spending-limits-config.yaml`

- [ ] **Per-agent custom limits configured**
  - VIP agents identified
  - Custom limits defined in config file
  - Limit changes require approval workflow

- [ ] **Spending limit violations logged**
  - All violations logged with audit trail
  - Alerts sent to security team
  - Investigation procedure documented

**Configuration:**

```yaml
# data/spending-limits-config.yaml
spendingLimits:
  default:
    maxTransactionSize: '1000000000' # 1000 USDC
    dailyLimit: '5000000000' # 5000 USDC
    monthlyLimit: '50000000000' # 50000 USDC
  perAgent:
    agent-vip-001:
      maxTransactionSize: '5000000000' # 5000 USDC
      dailyLimit: '25000000000' # 25000 USDC
      monthlyLimit: '250000000000' # 250000 USDC
```

---

### Fraud Detection

- [ ] **Suspicious activity detection enabled**
  - Rapid funding detection: >5 requests/hour
  - Unusual transaction detection: >3σ from mean
  - New token detection enabled
  - Automated wallet suspension configured

- [ ] **Epic 12 fraud detector integrated** (Future)
  - PlaceholderFraudDetector replaced with production detector
  - ML models deployed and tested
  - Fraud detection API configured
  - False positive rate acceptable (<5%)

- [ ] **Fraud alerts configured**
  - Real-time alerts to security team
  - Slack/PagerDuty integration
  - Alert escalation policy defined
  - 24/7 on-call rotation

**Verification:**

```bash
# Test fraud detection
npm test -- --testPathPattern=suspicious-activity-detector
# All tests should pass
```

---

### Audit Logging

- [ ] **Audit logging enabled for all operations**
  - All wallet operations logged
  - Dual storage: SQLite + Pino logs
  - Immutable trail (append-only)
  - Retention policy: 1 year minimum

- [ ] **Audit log fields complete**
  - Required fields: timestamp, operation, agentId, details, IP, userAgent, result
  - JSON format for structured parsing
  - Sensitive data redacted (no private keys)

- [ ] **Audit logs centralized**
  - Logs shipped to ELK/Splunk/Datadog
  - Real-time log aggregation
  - Searchable and queryable
  - Backup retention: 7 years (regulatory)

**Verification:**

```bash
# Check audit log entries
sqlite3 data/wallet/audit-log.db \
  "SELECT * FROM wallet_audit_log ORDER BY timestamp DESC LIMIT 10;"

# Verify log format
tail -100 logs/connector.log | grep '"audit":true'
```

---

### Key Protection

- [ ] **Private keys never logged**
  - Pino logger serializers configured
  - Wallet sanitization tested
  - Log monitoring for accidental exposure
  - Penetration test passed

- [ ] **Telemetry sanitization enabled**
  - `sanitizeWalletForTelemetry()` called before emission
  - No private keys in telemetry events
  - API responses never include keys

- [ ] **Security penetration test passed**
  - All test vectors passing
  - No vulnerabilities detected
  - Test report reviewed and approved

**Verification:**

```bash
# Run security penetration tests
npm test -- --testPathPattern=wallet-security-penetration
# All tests must pass

# Check for accidental key exposure in logs
grep -r "privateKey" logs/
# Should return no results
```

---

## Monitoring Checklist

### Telemetry and Metrics

- [ ] **Telemetry events flowing to dashboard**
  - Wallet creation events
  - Funding events
  - Payment channel events
  - Security events (rate limits, fraud)
  - Dashboard accessible to ops team

- [ ] **Balance tracking alerts configured**
  - Low balance warnings (<0.01 ETH)
  - Balance mismatch alerts
  - Funding failure alerts
  - Alert thresholds tuned for production load

- [ ] **Backup success/failure monitoring**
  - Backup completion alerts
  - Backup failure alerts (critical)
  - Backup integrity checks
  - Alert delivery tested

- [ ] **Rate limit violation alerts**
  - Real-time alerts on violations
  - Pattern detection (coordinated attacks)
  - Automatic IP blocking (optional)

- [ ] **Fraud detection alerts**
  - Suspicious activity alerts
  - Wallet suspension notifications
  - Fraud pattern reports (daily)

**Alert Configuration:**

```yaml
# alerts.yaml
alerts:
  low_eth_balance:
    threshold: '10000000000000000' # 0.01 ETH
    severity: warning
    channels: [email, slack]

  backup_failure:
    severity: critical
    channels: [pagerduty, email, slack]

  rate_limit_exceeded:
    severity: high
    channels: [slack, email]

  suspicious_activity:
    severity: critical
    channels: [pagerduty, slack]
```

---

### Health Monitoring

- [ ] **Application health checks configured**
  - `/health` endpoint implemented
  - Health check includes: database connectivity, RPC endpoints, master seed access
  - Kubernetes/ECS health probes configured
  - Unhealthy instances auto-restarted

- [ ] **RPC endpoint health monitoring**
  - Periodic connectivity tests
  - Response time monitoring
  - Automatic failover on failure
  - Alert on persistent failures

- [ ] **Database health monitoring**
  - SQLite connection pool monitoring
  - TigerBeetle cluster health
  - Disk space monitoring (alert at 80%)
  - Query performance monitoring

**Health Check Example:**

```typescript
app.get('/health', async (req, res) => {
  const health = {
    status: 'ok',
    timestamp: Date.now(),
    checks: {},
  };

  // Check database
  try {
    await db.query('SELECT 1');
    health.checks.database = 'ok';
  } catch (error) {
    health.checks.database = 'failed';
    health.status = 'degraded';
  }

  // Check RPC endpoints
  try {
    await provider.getBlockNumber();
    health.checks.rpc = 'ok';
  } catch (error) {
    health.checks.rpc = 'failed';
    health.status = 'degraded';
  }

  res.status(health.status === 'ok' ? 200 : 503).json(health);
});
```

---

### Performance Monitoring

- [ ] **Response time monitoring**
  - Wallet creation: <5 seconds
  - Balance query: <1 second (cached: <50ms)
  - Payment channel open: <30 seconds
  - Payment send: <1 second

- [ ] **Resource utilization monitoring**
  - CPU usage: <70% average
  - Memory usage: <2GB per instance
  - Disk I/O monitoring
  - Network bandwidth monitoring

- [ ] **Error rate monitoring**
  - Error rate: <1% of requests
  - 5xx errors: <0.1% of requests
  - Alert on error rate spike (>5%)

**Metrics to Track:**

```
- wallet_creation_duration_seconds
- balance_query_duration_seconds
- channel_open_duration_seconds
- payment_send_duration_seconds
- wallet_creation_errors_total
- balance_query_errors_total
- http_requests_total
- http_request_duration_seconds
```

---

## Testing Checklist

### Pre-Production Testing

- [ ] **Disaster recovery test completed**
  - Backup created in production-like environment
  - Server completely wiped
  - Backup restored successfully
  - All wallets recovered and verified
  - Recovery time documented (RTO: <4 hours target)

- [ ] **Security penetration test passed**
  - All test vectors passing
  - No critical or high vulnerabilities
  - Findings documented and remediated
  - Re-test after remediation

- [ ] **Load test: 1000+ agent wallets**
  - 1000+ wallets created successfully
  - All wallets funded
  - No performance degradation
  - Resource utilization within limits

- [ ] **End-to-end test: Full channel lifecycle**
  - Agent wallet created
  - Wallet funded successfully
  - Payment channel opened
  - Multiple payments sent
  - Channel closed and settled
  - Balances verified correct

- [ ] **Balance reconciliation test passed**
  - Create wallet and fund
  - Perform transactions
  - Backup and restore
  - Balances match on-chain data

**Load Test Script:**

```bash
#!/bin/bash
# Load test: Create 1000 agent wallets

echo "Starting load test: Creating 1000 wallets"

for i in {1..1000}; do
  curl -X POST http://localhost:3000/api/wallets \
    -H "Content-Type: application/json" \
    -d "{\"agentId\":\"load-test-agent-$i\"}" &

  # Respect rate limits (100/hour)
  if [ $((i % 100)) -eq 0 ]; then
    wait  # Wait for batch to complete
    echo "Batch $((i/100)) complete, waiting for rate limit reset"
    sleep 3600  # 1 hour
  fi
done

wait
echo "Load test complete: 1000 wallets created"
```

---

### Staging Environment Testing

- [ ] **Staging environment mirrors production**
  - Same infrastructure configuration
  - Same security settings
  - Same monitoring/alerting setup
  - Separate RPC endpoints (testnet or separate accounts)

- [ ] **Full deployment tested in staging**
  - Deployment procedure documented
  - Deployment tested multiple times
  - Rollback procedure tested
  - Zero-downtime deployment verified

- [ ] **Integration tests passing in staging**
  - All integration tests green
  - No flaky tests
  - Test coverage >80%
  - Performance benchmarks met

**Verification:**

```bash
# Run full integration test suite in staging
npm test -- --testPathPattern=integration
# All tests must pass

# Run performance benchmarks
npm run benchmark
# All benchmarks must meet targets
```

---

## Operational Checklist

### Runbooks and Procedures

- [ ] **Backup restore procedure documented**
  - Step-by-step instructions
  - Tested in staging
  - Recovery time objective (RTO): 4 hours
  - Recovery point objective (RPO): 24 hours

- [ ] **Incident response plan documented**
  - Escalation procedures
  - On-call rotation defined
  - Contact information up-to-date
  - Incident severity definitions

- [ ] **Master seed compromise response plan**
  - Immediate actions defined
  - Communication plan
  - Fund migration procedure
  - Post-incident review process

- [ ] **Wallet suspension procedure documented**
  - When to suspend wallets
  - How to suspend wallets
  - Investigation checklist
  - Reactivation approval workflow

- [ ] **Spending limit adjustment procedure**
  - Request process
  - Approval requirements (multi-person authorization)
  - Documentation requirements
  - Audit trail

---

### Team Readiness

- [ ] **Operations team trained**
  - Training on wallet operations
  - Runbook walkthrough
  - Incident response drills
  - Access to all necessary tools

- [ ] **On-call rotation established**
  - 24/7 coverage
  - Primary and backup on-call
  - Escalation chain defined
  - PagerDuty/Opsgenie configured

- [ ] **Access permissions configured**
  - Production access restricted
  - SSH keys/certificates issued
  - MFA enabled for all access
  - Access reviews scheduled

- [ ] **Communication channels established**
  - Slack channel for alerts
  - Email lists configured
  - Status page setup (if customer-facing)
  - Incident communication plan

---

### Compliance and Documentation

- [ ] **Compliance requirements met**
  - Data retention policies: 7 years for audit logs
  - PCI-DSS compliance (if handling fiat)
  - GDPR compliance (if EU users)
  - Regulatory reporting procedures

- [ ] **Documentation complete**
  - Architecture documentation
  - API documentation
  - Security documentation
  - Troubleshooting guides
  - FAQ

- [ ] **Change management process**
  - Change approval workflow
  - Change log maintained
  - Deployment schedule published
  - Rollback plan for all changes

- [ ] **Security audit completed**
  - External security audit
  - Findings remediated
  - Audit report approved
  - Re-audit scheduled (annual)

---

## Final Verification

### Pre-Deployment Checklist

Complete this final checklist immediately before production deployment:

- [ ] **All previous checklist items completed** ✅
- [ ] **Production configuration reviewed**
  - All environment variables set correctly
  - Secrets stored in secrets manager (not code)
  - Debug logging disabled
  - Error tracking configured (Sentry, etc.)

- [ ] **Deployment plan reviewed**
  - Deployment window scheduled
  - Stakeholders notified
  - Rollback plan ready
  - Deployment tested in staging

- [ ] **Monitoring validated**
  - All dashboards accessible
  - All alerts firing to correct channels
  - Test alert sent and received

- [ ] **Communication prepared**
  - Internal announcement draft
  - Customer communication (if applicable)
  - Status page ready
  - Support team briefed

- [ ] **Post-deployment plan ready**
  - Smoke tests defined
  - Success criteria documented
  - Monitoring checklist for first 24 hours
  - Go/no-go decision criteria

---

## Post-Deployment Verification

Complete within 24 hours of production deployment:

- [ ] **Smoke tests passing**
  - Create test wallet
  - Fund test wallet
  - Open test channel
  - Send test payment
  - Close test channel
  - All operations successful

- [ ] **Monitoring operational**
  - Metrics flowing to dashboard
  - Alerts functioning correctly
  - No critical errors in logs
  - Performance within SLAs

- [ ] **Security verification**
  - No security alerts triggered
  - Audit logs capturing all operations
  - No private keys in logs
  - Rate limiting functioning

- [ ] **Backup verification**
  - First automated backup completed
  - Backup stored in all locations
  - Backup encryption verified
  - Test restore performed

- [ ] **Team notified**
  - Deployment complete announcement
  - Known issues documented
  - Support team enabled
  - On-call engineer briefed

---

## Continuous Verification

Schedule these regular verification tasks:

### Daily

- Check for failed backups
- Review security alerts
- Monitor error rates
- Check disk space

### Weekly

- Review audit logs for anomalies
- Test RPC endpoint failover
- Review spending limit violations
- Check for software updates

### Monthly

- Test backup restore
- Review and update spending limits
- Security log review
- Performance benchmarking

### Quarterly

- Full disaster recovery drill
- Access permission review
- Security posture review
- Update documentation

### Annually

- External security audit
- Password rotation
- Review and update procedures
- Team training refresh

---

## Deployment Sign-Off

**Required Approvals:**

| Role          | Name               | Signature          | Date     |
| ------------- | ------------------ | ------------------ | -------- |
| Tech Lead     | **\*\***\_**\*\*** | **\*\***\_**\*\*** | **\_\_** |
| Security Lead | **\*\***\_**\*\*** | **\*\***\_**\*\*** | **\_\_** |
| DevOps Lead   | **\*\***\_**\*\*** | **\*\***\_**\*\*** | **\_\_** |
| Product Owner | **\*\***\_**\*\*** | **\*\***\_**\*\*** | **\_\_** |

**Deployment Authorization:**

- [ ] All checklist items completed
- [ ] All required approvals obtained
- [ ] Deployment window confirmed
- [ ] Rollback plan ready
- [ ] **AUTHORIZED TO DEPLOY TO PRODUCTION**

**Deployment Date:** **\*\*\*\***\_\_\_**\*\*\*\***
**Deployed By:** **\*\*\*\***\_\_\_**\*\*\*\***
**Deployment Time:** **\*\*\*\***\_\_\_**\*\*\*\***

---

## Support Resources

- **Integration Guide**: [Agent Wallet Integration](../guides/agent-wallet-integration.md)
- **API Reference**: [Agent Wallet API](../api/agent-wallet-api.md)
- **Security**: [Security Best Practices](../security/agent-wallet-security.md)
- **Troubleshooting**: [Troubleshooting Guide](../guides/agent-wallet-troubleshooting.md)
- **FAQ**: [Frequently Asked Questions](../guides/agent-wallet-faq.md)

**Emergency Contacts:**

- **On-Call Engineer**: [PagerDuty]
- **Security Team**: security@interledger.org
- **DevOps Team**: devops@interledger.org

---

**Version:** 1.0
**Last Updated:** 2026-01-21
**Next Review:** 2026-04-21 (Quarterly)
