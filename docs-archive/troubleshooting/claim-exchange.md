# Claim Exchange Troubleshooting Guide

**Story:** 17.6 - Telemetry and Monitoring
**Epic:** 17 - BTP Off-Chain Claim Exchange Protocol

This guide helps operators diagnose and resolve issues with the BTP claim exchange protocol.

---

## Table of Contents

1. [Common Issues](#common-issues)
   - [Claims Not Being Sent](#claims-not-being-sent)
   - [Claims Failing Verification](#claims-failing-verification)
   - [Claims Not Being Redeemed](#claims-not-being-redeemed)
2. [Debugging Steps](#debugging-steps)
3. [Prometheus Metrics Reference](#prometheus-metrics-reference)
4. [Alert Response Procedures](#alert-response-procedures)

---

## Common Issues

### Claims Not Being Sent

**Symptoms:**

- Low or zero `claims_sent_total` metrics
- No CLAIM_SENT telemetry events in Explorer UI
- Settlement balance increasing but no claim activity

**Possible Causes:**

1. **BTP Connection Issues**
   - Check BTP connection status: `claims_sent_total{success="false"}` metric
   - Verify WebSocket connection health between peers
   - Review BTP server/client logs for connection errors

2. **Settlement Threshold Not Reached**
   - Verify settlement threshold configuration in `config.yaml`
   - Check account balance metrics: `account_balance_units`
   - Settlement may not trigger if balance is below threshold

3. **PerPacketClaimService Not Running**
   - Check if PerPacketClaimService is properly initialized in connector (requires ChannelManager + PaymentChannelSDK)
   - Review startup logs for `per_packet_claims_enabled` initialization message
   - Verify claim signer configuration (EVM keys)

**Resolution Steps:**

```bash
# 1. Check BTP connection health
curl http://localhost:8080/health | jq '.connector.peers'

# 2. Check claim send metrics
curl http://localhost:8080/metrics | grep claims_sent_total

# 3. Check connector logs for PerPacketClaimService errors
docker logs connector-node-0 2>&1 | grep per-packet-claim

# 4. Verify settlement threshold in config
cat config/topology.yaml | grep settlementThreshold

# 5. Check account balance
curl http://localhost:8080/metrics | grep account_balance_units
```

---

### Claims Failing Verification

**Symptoms:**

- High `claim_verification_failures_total` metric
- CLAIM_RECEIVED events with `verified: false` in Explorer UI
- Prometheus alert: `HighClaimVerificationFailureRate`

**Possible Causes:**

1. **Invalid Signature** (`error_type="invalid_signature"`)
   - Claim signer mismatch between sender and receiver
   - Incorrect claim signer address configured for peer
   - Key rotation without peer notification

2. **Non-Monotonic Nonce** (`error_type="non_monotonic_nonce"`)
   - Previous claim with higher nonce was already processed
   - Clock drift between nodes causing timestamp issues
   - Claim database state corruption or rollback

3. **Channel ID Mismatch**
   - Sender referencing non-existent or closed channel
   - Channel state not synchronized between peers

**Resolution Steps:**

```bash
# 1. Check verification failure metrics by error type
curl http://localhost:8080/metrics | grep claim_verification_failures_total

# 2. View recent CLAIM_RECEIVED events in Explorer UI
# Navigate to http://localhost:9100 and filter by "CLAIM_RECEIVED"

# 3. Check claim signer configuration for peer
cat config/topology.yaml | grep -A 10 "peer-id: <PEER_ID>"

# 4. Verify claim signer addresses match on both nodes
# On sender:
curl http://localhost:8080/api/claim-signer/evm | jq '.address'
# On receiver:
curl http://<PEER_IP>:8080/api/claim-signer/evm | jq '.address'

# 5. Check ClaimReceiver logs for specific error details
docker logs connector-node-0 2>&1 | grep ClaimReceiver | grep verified=false
```

**Invalid Signature Fix:**

1. Verify correct claim signer public key is configured in receiver's peer config
2. Restart both nodes if configuration was updated
3. Test with manual claim send to verify signature validation

---

### Claims Not Being Redeemed

**Symptoms:**

- CLAIM_RECEIVED events with `verified: true` exist
- No corresponding CLAIM_REDEEMED events
- Prometheus alert: `ClaimRedemptionStalled`

**Possible Causes:**

1. **ClaimRedemptionService Not Running**
   - Service not initialized during startup
   - Service crashed due to unhandled error

2. **Profitability Threshold Not Met**
   - Claim amount too low to cover gas costs
   - Gas prices spiked making redemption unprofitable
   - Check `gasCost` vs `amount` in telemetry events

3. **Blockchain RPC Connectivity Issues**
   - RPC node down or unreachable
   - Rate limiting from RPC provider
   - Network connectivity problems

4. **Insufficient Gas/Balance**
   - Node wallet lacks sufficient native token for gas
   - EVM account out of ETH for gas

**Resolution Steps:**

```bash
# 1. Check last redemption timestamp
curl http://localhost:8080/metrics | grep claim_last_redemption_timestamp_seconds

# 2. Check ClaimRedemptionService logs
docker logs connector-node-0 2>&1 | grep ClaimRedemptionService

# 3. Verify RPC connectivity
# For EVM (Base):
curl https://mainnet.base.org -X POST -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}'

# 4. Check node wallet balances
curl http://localhost:8080/api/balances | jq '.ethBalance'

# 5. Review profitability calculations in logs
docker logs connector-node-0 2>&1 | grep "profitability check"
```

**Stalled Redemption Fix:**

1. Restart ClaimRedemptionService (restart connector)
2. Increase gas price threshold in config if gas prices are high
3. Fund node wallet with native tokens if balance is low
4. Switch to backup RPC endpoint if primary is failing

---

## Debugging Steps

### Step 1: Check Telemetry Events in Explorer UI

1. Navigate to Explorer UI: `http://localhost:9100`
2. Use filter bar to show only claim events: `CLAIM_SENT`, `CLAIM_RECEIVED`, `CLAIM_REDEEMED`
3. Click on a claim event to view details and timeline
4. Look for `messageId` to correlate events across lifecycle

### Step 2: Review Prometheus Metrics

Query Prometheus for claim-specific metrics:

```promql
# Claim send success rate over 5 minutes
rate(claims_sent_total{success="true"}[5m]) /
rate(claims_sent_total[5m])

# Claim verification failure rate by error type
sum by (error_type) (rate(claim_verification_failures_total[5m]))

# Claim redemption latency p99
histogram_quantile(0.99, rate(claim_redemption_latency_seconds_bucket[5m]))

# Time since last redemption per blockchain
time() - claim_last_redemption_timestamp_seconds
```

### Step 3: Check Logs with Correlation by messageId

Claims can be tracked across their entire lifecycle using `messageId`:

```bash
# Find all log entries for a specific messageId
docker logs connector-node-0 2>&1 | grep "msg_abc123"

# Track claim lifecycle in order
docker logs connector-node-0 2>&1 | grep "msg_abc123" | grep -E "CLAIM_(SENT|RECEIVED|REDEEMED)"
```

### Step 4: Verify Blockchain Node Connectivity

```bash
# Test EVM node connectivity
cast block-number --rpc-url https://mainnet.base.org
```

---

## Prometheus Metrics Reference

### Claim Send Metrics

| Metric              | Type    | Labels                             | Description                |
| ------------------- | ------- | ---------------------------------- | -------------------------- |
| `claims_sent_total` | Counter | `peer_id`, `blockchain`, `success` | Total claims sent to peers |

### Claim Receive Metrics

| Metric                              | Type    | Labels                                | Description                       |
| ----------------------------------- | ------- | ------------------------------------- | --------------------------------- |
| `claims_received_total`             | Counter | `peer_id`, `blockchain`, `verified`   | Total claims received from peers  |
| `claim_verification_failures_total` | Counter | `peer_id`, `blockchain`, `error_type` | Total claim verification failures |

### Claim Redemption Metrics

| Metric                                    | Type      | Labels                  | Description                       |
| ----------------------------------------- | --------- | ----------------------- | --------------------------------- |
| `claims_redeemed_total`                   | Counter   | `blockchain`, `success` | Total claims redeemed on-chain    |
| `claim_redemption_latency_seconds`        | Histogram | `blockchain`            | Time from receipt to redemption   |
| `claim_last_redemption_timestamp_seconds` | Gauge     | `blockchain`            | Unix timestamp of last redemption |

### Example Queries

```promql
# Claim send success rate by blockchain
sum by (blockchain) (rate(claims_sent_total{success="true"}[5m]))
/ sum by (blockchain) (rate(claims_sent_total[5m]))

# Total verification failures by error type
sum by (error_type) (increase(claim_verification_failures_total[1h]))

# Average redemption latency by blockchain
rate(claim_redemption_latency_seconds_sum[5m])
/ rate(claim_redemption_latency_seconds_count[5m])
```

---

## Alert Response Procedures

### HighClaimSendFailureRate

**Severity:** Warning
**Response Time:** 15 minutes

1. Check BTP connection status between affected peer
2. Review PerPacketClaimService logs for specific error messages (`grep per-packet-claim`)
3. Verify peer is reachable and accepting connections
4. If widespread, check network connectivity
5. Verify ChannelManager has channels for affected peers

### HighClaimVerificationFailureRate

**Severity:** Warning
**Response Time:** 15 minutes

1. Check `error_type` label to identify failure mode
2. For `invalid_signature`: Verify claim signer configuration matches
3. For `non_monotonic_nonce`: Check for clock drift or state corruption
4. Review recent configuration changes
5. Contact peer operator if signature mismatch suspected

### ClaimRedemptionStalled

**Severity:** Critical
**Response Time:** 5 minutes

1. Check ClaimRedemptionService health immediately
2. Verify RPC node connectivity for affected blockchain
3. Check node wallet balance for gas funds
4. Review recent errors in ClaimRedemptionService logs
5. Restart service if necessary
6. Monitor for successful redemptions after restart

### ClaimRedemptionFailures

**Severity:** Warning
**Response Time:** 15 minutes

1. Check gas prices for affected blockchain
2. Verify RPC node health and rate limits
3. Review on-chain transaction failures if tx hashes available
4. Check claim signature validity on-chain
5. Increase gas price settings if network is congested
6. Switch to backup RPC if primary is failing

---

## Related Documentation

- [Epic 17: BTP Claim Exchange Protocol](../prd/epic-17-btp-claim-exchange.md)
- [Story 17.1: BTP Claim Message Protocol](../stories/17.1.story.md)
- [Story 17.5: Automatic Claim Redemption](../stories/17.5.story.md)
- [Prometheus Alert Rules](../../monitoring/prometheus/claim-alert-rules.yml)
- [Architecture: Settlement Layer](../architecture/settlement.md)

---

## Support

For additional support:

1. Check GitHub issues: https://github.com/your-org/m2m/issues
2. Join Discord: https://discord.gg/your-server
3. Email: support@example.com
