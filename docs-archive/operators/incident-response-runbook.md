# ILP Connector Incident Response Runbook

This runbook provides step-by-step guidance for responding to alerts and incidents affecting the ILP Connector infrastructure.

## Table of Contents

- [Alert Reference](#alert-reference)
- [High Packet Error Rate](#high-packet-error-rate)
- [Settlement Failures](#settlement-failures)
- [TigerBeetle Unavailable](#tigerbeetle-unavailable)
- [Channel Dispute](#channel-dispute)
- [High P99 Latency](#high-p99-latency)
- [Low Throughput](#low-throughput)
- [Connector Down](#connector-down)
- [High Memory Usage](#high-memory-usage)
- [Critical Error Spike](#critical-error-spike)
- [SLA Breach](#sla-breach)
- [Escalation Procedures](#escalation-procedures)

---

## Alert Reference

| Alert Name             | Severity | Threshold      | Duration  |
| ---------------------- | -------- | -------------- | --------- |
| HighPacketErrorRate    | warning  | >5% error rate | 2m        |
| SettlementFailures     | critical | any failure    | 1m        |
| TigerBeetleUnavailable | critical | service down   | 1m        |
| ChannelDispute         | high     | any dispute    | immediate |
| HighP99Latency         | warning  | >10ms          | 5m        |
| LowThroughput          | warning  | <1000 TPS      | 5m        |
| ConnectorDown          | critical | service down   | 1m        |
| HighMemoryUsage        | warning  | >85%           | 5m        |
| CriticalErrorSpike     | critical | >10 errors/min | 1m        |
| SettlementSLABreach    | warning  | <99% success   | 5m        |
| PacketSLABreach        | warning  | <99.9% success | 5m        |

---

## High Packet Error Rate

**Alert:** `HighPacketErrorRate`
**Severity:** Warning
**Condition:** Error rate > 5% for 2 minutes

### Symptoms

- Increased packet rejections
- Higher than normal error rates in logs
- Possible peer connectivity issues

### Diagnosis Steps

1. **Check error distribution by type:**

   ```bash
   # Query Prometheus for error breakdown
   curl -s 'http://prometheus:9090/api/v1/query?query=rate(ilp_packets_processed_total{status="error"}[5m])' | jq
   ```

2. **Review recent connector logs:**

   ```bash
   # Filter for error-level logs
   docker logs agent-runtime --since 10m 2>&1 | grep '"level":"error"'
   ```

3. **Check peer connection status:**

   ```bash
   curl -s http://connector:8080/health | jq '.peersConnected'
   ```

4. **Identify affected destinations:**
   ```bash
   # Check if errors are concentrated on specific routes
   docker logs agent-runtime --since 10m 2>&1 | grep 'T0[0-9]_' | head -20
   ```

### Resolution Steps

1. **If peer connection issues:**
   - Verify network connectivity to peer: `ping peer-hostname`
   - Check peer's health endpoint: `curl http://peer:8080/health`
   - Review authentication tokens in configuration
   - Restart BTP connection if needed

2. **If routing issues:**
   - Verify routing table configuration
   - Check if destination prefixes are correctly mapped
   - Review route priorities

3. **If resource constraints:**
   - Check memory usage: `docker stats agent-runtime`
   - Check CPU usage
   - Consider scaling horizontally

4. **If TigerBeetle issues:**
   - Check TigerBeetle connectivity (see [TigerBeetle Unavailable](#tigerbeetle-unavailable))

### Post-Incident

- Update runbook if new failure mode discovered
- Review error rate trends over past week
- Consider adjusting alert threshold if false positive

---

## Settlement Failures

**Alert:** `SettlementFailures`
**Severity:** Critical
**Condition:** Any settlement failure for 1 minute

### Symptoms

- Settlement operations failing
- Balance discrepancies
- Packets rejected with T00_INTERNAL_ERROR

### Diagnosis Steps

1. **Check settlement error logs:**

   ```bash
   docker logs agent-runtime --since 10m 2>&1 | grep -E 'settlement|Settlement'
   ```

2. **Verify TigerBeetle connectivity:**

   ```bash
   curl -s http://connector:8080/health | jq '.dependencies.tigerbeetle'
   ```

3. **Check settlement metrics:**

   ```bash
   curl -s 'http://prometheus:9090/api/v1/query?query=settlements_executed_total{status="failure"}' | jq
   ```

4. **Review account balances:**
   ```bash
   # Check for unusual balance patterns
   curl -s 'http://prometheus:9090/api/v1/query?query=account_balance_units' | jq
   ```

### Resolution Steps

1. **If TigerBeetle connection issue:**
   - Verify TigerBeetle cluster health
   - Check network connectivity to TigerBeetle replicas
   - Review TigerBeetle logs for errors

2. **If account validation errors:**
   - Verify peer accounts exist in TigerBeetle
   - Check account IDs match configuration
   - Review credit limits (may need adjustment)

3. **If credit limit exceeded:**
   - Check current balance against credit limit
   - Consider triggering manual settlement
   - Adjust credit limits if appropriate

4. **If blockchain settlement failing:**
   - Check blockchain RPC connectivity
   - Verify wallet has sufficient funds for gas
   - Check for pending transactions

### Post-Incident

- Document root cause
- Review settlement threshold configuration
- Consider adding more specific alerting

---

## TigerBeetle Unavailable

**Alert:** `TigerBeetleUnavailable`
**Severity:** Critical
**Condition:** TigerBeetle down for 1 minute

### Symptoms

- Health check shows TigerBeetle as "down"
- All packet forwarding failing
- Settlement operations failing

### Diagnosis Steps

1. **Check TigerBeetle container status:**

   ```bash
   docker ps | grep tigerbeetle
   docker logs tigerbeetle --tail 100
   ```

2. **Verify network connectivity:**

   ```bash
   # From connector container
   docker exec agent-runtime nc -zv tigerbeetle 3000
   ```

3. **Check TigerBeetle cluster health:**

   ```bash
   # If using multiple replicas
   for replica in tb-1 tb-2 tb-3; do
     echo "Checking $replica..."
     docker logs $replica --tail 20
   done
   ```

4. **Review connector health endpoint:**
   ```bash
   curl -s http://connector:8080/health | jq '.dependencies.tigerbeetle'
   ```

### Resolution Steps

1. **If TigerBeetle container crashed:**

   ```bash
   # Restart TigerBeetle
   docker restart tigerbeetle

   # Wait for startup
   sleep 10

   # Verify health
   curl -s http://connector:8080/health | jq '.dependencies.tigerbeetle'
   ```

2. **If network issue:**
   - Verify Docker network configuration
   - Check DNS resolution
   - Review firewall rules

3. **If disk space issue:**

   ```bash
   df -h /var/lib/tigerbeetle
   # Clean up old data if needed (CAUTION: consult TigerBeetle docs)
   ```

4. **If cluster split-brain:**
   - Identify the primary replica
   - Restart minority replicas
   - Monitor for quorum recovery

### Post-Incident

- Review TigerBeetle backup procedures
- Consider adding more replicas for HA
- Update monitoring for disk space

---

## Channel Dispute

**Alert:** `ChannelDispute`
**Severity:** High
**Condition:** Any payment channel in disputed state

### Symptoms

- Payment channel showing "disputed" status
- Potential fund recovery in progress
- Affected peer may be malicious or experiencing issues

### Diagnosis Steps

1. **Identify disputed channels:**

   ```bash
   curl -s 'http://prometheus:9090/api/v1/query?query=payment_channels_active{status="disputed"}' | jq
   ```

2. **Check channel dispute logs:**

   ```bash
   docker logs agent-runtime --since 1h 2>&1 | grep -i dispute
   ```

3. **Review channel details:**
   - Check EVM contract state on Base block explorer

4. **Identify the counterparty:**
   ```bash
   # Review recent channel operations for the peer
   docker logs agent-runtime --since 1h 2>&1 | grep 'channel' | grep 'peer-id'
   ```

### Resolution Steps

1. **Assess the dispute:**
   - Determine if dispute is legitimate (counterparty failure) or malicious
   - Review recent claim amounts vs. actual state
   - Gather evidence of latest signed state

2. **If legitimate dispute (counterparty failure):**
   - Allow dispute period to complete
   - Submit counter-claim with latest state if needed
   - Monitor for dispute resolution

3. **If malicious dispute:**
   - Immediately submit counter-claim with valid state
   - Document evidence for potential legal action
   - Consider blacklisting peer after resolution

4. **Monitor EVM channel dispute:**

   ```bash
   # Monitor dispute on Base explorer
   # Contract will resolve after dispute period
   ```

### Post-Incident

- Review peer trust levels
- Update fraud detection rules if needed
- Document dispute outcome

---

## High P99 Latency

**Alert:** `HighP99Latency`
**Severity:** Warning
**Condition:** P99 latency > 10ms for 5 minutes

### Symptoms

- Slow packet processing
- Timeouts in upstream services
- Degraded user experience

### Diagnosis Steps

1. **Check latency distribution:**

   ```bash
   curl -s 'http://prometheus:9090/api/v1/query?query=histogram_quantile(0.99,rate(ilp_packet_latency_seconds_bucket[5m]))' | jq
   ```

2. **Check resource utilization:**

   ```bash
   docker stats agent-runtime --no-stream
   ```

3. **Review TigerBeetle latency:**

   ```bash
   curl -s http://connector:8080/health | jq '.dependencies.tigerbeetle.latencyMs'
   ```

4. **Check for garbage collection pauses:**
   ```bash
   docker logs agent-runtime --since 10m 2>&1 | grep -i 'gc\|garbage'
   ```

### Resolution Steps

1. **If CPU bound:**
   - Scale horizontally (add more connector instances)
   - Optimize hot paths in packet processing
   - Review performance configuration

2. **If memory pressure:**
   - Increase container memory limits
   - Review for memory leaks
   - Reduce batch sizes

3. **If TigerBeetle latency:**
   - Check TigerBeetle cluster health
   - Review disk I/O on TigerBeetle nodes
   - Consider SSD storage for TigerBeetle

4. **If network latency:**
   - Check network path to peers
   - Consider geographic distribution
   - Review MTU settings

### Post-Incident

- Profile connector performance
- Review SLA targets
- Update capacity planning

---

## Low Throughput

**Alert:** `LowThroughput`
**Severity:** Warning
**Condition:** Throughput < 1000 TPS for 5 minutes

### Symptoms

- Reduced packet processing rate
- Possible upstream service degradation
- May indicate partial failure

### Diagnosis Steps

1. **Check current throughput:**

   ```bash
   curl -s 'http://prometheus:9090/api/v1/query?query=rate(ilp_packets_processed_total[5m])' | jq
   ```

2. **Compare to historical baseline:**

   ```bash
   # Check throughput over past hour
   curl -s 'http://prometheus:9090/api/v1/query_range?query=rate(ilp_packets_processed_total[5m])&start=-1h&step=1m' | jq
   ```

3. **Check peer connectivity:**

   ```bash
   curl -s http://connector:8080/health | jq '.peersConnected, .totalPeers'
   ```

4. **Review for errors affecting throughput:**
   ```bash
   docker logs agent-runtime --since 10m 2>&1 | grep '"level":"error"' | wc -l
   ```

### Resolution Steps

1. **If peer connectivity issue:**
   - Verify all peers are connected
   - Check peer health endpoints
   - Restart failed peer connections

2. **If resource constraints:**
   - Scale horizontally
   - Increase resource limits
   - Optimize configuration

3. **If external dependency slow:**
   - Check TigerBeetle performance
   - Check blockchain RPC responsiveness
   - Review connection pool utilization

4. **If legitimate traffic reduction:**
   - Verify with business stakeholders
   - Adjust alert threshold if needed
   - Document expected traffic patterns

### Post-Incident

- Review capacity planning
- Update traffic baseline
- Consider auto-scaling

---

## Connector Down

**Alert:** `ConnectorDown`
**Severity:** Critical
**Condition:** Connector health endpoint unreachable for 1 minute

### Symptoms

- Health endpoint returning errors or timeout
- No packet processing
- Peer connections may be failing

### Diagnosis Steps

1. **Check container status:**

   ```bash
   docker ps | grep agent-runtime
   docker logs agent-runtime --tail 100
   ```

2. **Check for OOM kill:**

   ```bash
   dmesg | grep -i 'killed process'
   docker inspect agent-runtime | jq '.[0].State'
   ```

3. **Check system resources:**

   ```bash
   docker stats --no-stream
   df -h
   ```

4. **Check port bindings:**
   ```bash
   netstat -tlnp | grep -E '3000|8080'
   ```

### Resolution Steps

1. **If container crashed:**

   ```bash
   # Check logs for crash reason
   docker logs agent-runtime --tail 200

   # Restart container
   docker restart agent-runtime

   # Monitor startup
   docker logs -f agent-runtime
   ```

2. **If OOM killed:**
   - Increase memory limits in docker-compose
   - Review for memory leaks
   - Check for excessive connection pooling

3. **If port conflict:**
   - Identify conflicting process
   - Update port configuration
   - Restart with correct ports

4. **If configuration error:**
   - Review configuration files
   - Validate YAML syntax
   - Check environment variables

### Post-Incident

- Implement automatic restart policy
- Review resource limits
- Add pre-start validation

---

## High Memory Usage

**Alert:** `HighMemoryUsage`
**Severity:** Warning
**Condition:** Memory usage > 85% for 5 minutes

### Symptoms

- Container using high memory
- Potential OOM risk
- Possible memory leak

### Diagnosis Steps

1. **Check current memory usage:**

   ```bash
   docker stats agent-runtime --no-stream
   ```

2. **Check memory trend:**

   ```bash
   curl -s 'http://prometheus:9090/api/v1/query_range?query=process_resident_memory_bytes&start=-1h&step=1m' | jq
   ```

3. **Generate heap dump (if supported):**

   ```bash
   docker exec agent-runtime node --inspect --expose-gc -e "global.gc()"
   ```

4. **Check for connection leaks:**
   ```bash
   docker exec agent-runtime netstat -an | grep ESTABLISHED | wc -l
   ```

### Resolution Steps

1. **If memory leak suspected:**
   - Restart container to reclaim memory
   - Enable heap profiling
   - Review recent code changes

2. **If legitimate memory growth:**
   - Increase container memory limits
   - Review batch sizes and buffer configurations
   - Optimize caching strategies

3. **If connection pooling issue:**
   - Review connection pool configuration
   - Check for connection leaks
   - Reduce pool sizes if excessive

### Post-Incident

- Profile memory usage patterns
- Implement memory leak detection
- Update resource limits

---

## Critical Error Spike

**Alert:** `CriticalErrorSpike`
**Severity:** Critical
**Condition:** > 10 critical errors per minute for 1 minute

### Symptoms

- High volume of critical errors
- Likely affecting service availability
- May indicate systemic issue

### Diagnosis Steps

1. **Identify error types:**

   ```bash
   docker logs agent-runtime --since 5m 2>&1 | grep '"level":"error"' | jq -r '.err.type // .err // .msg' | sort | uniq -c | sort -rn
   ```

2. **Check for patterns:**

   ```bash
   # Review timestamps for error clustering
   docker logs agent-runtime --since 5m 2>&1 | grep '"level":"error"' | jq -r '.time' | cut -d: -f1-2 | uniq -c
   ```

3. **Review dependency health:**
   ```bash
   curl -s http://connector:8080/health | jq '.dependencies'
   ```

### Resolution Steps

1. Follow specific runbook based on error type identified
2. If unknown error type:
   - Capture full stack trace
   - Check for recent deployments
   - Consider rollback if recent change

### Post-Incident

- Root cause analysis
- Update error handling
- Add specific alerting for new error type

---

## SLA Breach

**Alert:** `SettlementSLABreach` or `PacketSLABreach`
**Severity:** Warning
**Condition:** Success rate below threshold for 5 minutes

### Symptoms

- Service level degradation
- Customer impact likely
- May trigger contractual obligations

### Diagnosis Steps

1. **Check current SLA metrics:**

   ```bash
   curl -s http://connector:8080/health | jq '.sla'
   ```

2. **Identify contributing factors:**
   - Review error rate alerts
   - Check latency alerts
   - Review dependency health

3. **Calculate impact window:**
   ```bash
   # Query for SLA metric history
   curl -s 'http://prometheus:9090/api/v1/query_range?query=rate(ilp_packets_processed_total{status="fulfilled"}[5m])/rate(ilp_packets_processed_total[5m])&start=-1h&step=1m' | jq
   ```

### Resolution Steps

1. Address underlying issue (refer to specific alert runbooks)
2. Document incident timeline
3. Notify stakeholders if customer-impacting
4. Prepare incident report

### Post-Incident

- Calculate total SLA impact
- Review SLA thresholds
- Update capacity planning

---

## Escalation Procedures

### Severity Levels

| Level    | Response Time | Examples                                              |
| -------- | ------------- | ----------------------------------------------------- |
| Critical | Immediate     | TigerBeetle down, Connector down, Settlement failures |
| High     | 15 minutes    | Channel disputes                                      |
| Warning  | 1 hour        | High error rate, High latency, SLA breach             |

### Escalation Path

1. **L1 - On-Call Engineer**
   - Initial triage
   - Follow runbook procedures
   - Escalate if not resolved in 30 minutes

2. **L2 - Platform Team Lead**
   - Complex technical issues
   - Cross-service coordination
   - Escalate if not resolved in 1 hour

3. **L3 - Engineering Management**
   - Customer-impacting incidents
   - Policy decisions
   - External communication

### Communication Channels

- **Slack:** #agent-runtime-alerts
- **PagerDuty:** ILP Connector Service
- **Status Page:** Update for customer-visible incidents

### Incident Documentation

For every critical incident, create an incident report including:

1. Timeline of events
2. Root cause analysis
3. Impact assessment
4. Resolution steps taken
5. Preventive measures

---

## Appendix: Useful Commands

### Prometheus Queries

```promql
# Packet success rate
sum(rate(ilp_packets_processed_total{status="fulfilled"}[5m])) / sum(rate(ilp_packets_processed_total[5m]))

# Settlement success rate
sum(rate(settlements_executed_total{status="success"}[5m])) / sum(rate(settlements_executed_total[5m]))

# P99 latency
histogram_quantile(0.99, rate(ilp_packet_latency_seconds_bucket[5m]))

# Error rate by type
rate(connector_errors_total[5m])

# Active channels by status
payment_channels_active
```

### Log Analysis

```bash
# Count errors by type
docker logs agent-runtime --since 1h 2>&1 | grep '"level":"error"' | jq -r '.msg' | sort | uniq -c | sort -rn

# Find correlation IDs for tracking
docker logs agent-runtime --since 1h 2>&1 | grep 'pkt_' | head -20

# Extract packet flow
docker logs agent-runtime --since 1h 2>&1 | jq 'select(.correlationId) | {time, correlationId, msg}'
```

### Health Checks

```bash
# Full health status
curl -s http://connector:8080/health | jq

# Liveness probe
curl -s http://connector:8080/health/live

# Readiness probe
curl -s http://connector:8080/health/ready

# Prometheus metrics
curl -s http://connector:8080/metrics | head -50
```
