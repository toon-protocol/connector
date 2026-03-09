# API Reference

This document provides comprehensive API reference for all operator endpoints exposed by the M2M ILP Connector, including health checks, metrics, and administrative endpoints.

## Table of Contents

1. [Overview](#overview)
2. [Health Check Endpoints](#health-check-endpoints)
3. [Metrics Endpoint](#metrics-endpoint)
4. [Response Schemas](#response-schemas)
5. [Authentication](#authentication)
6. [Rate Limiting](#rate-limiting)
7. [Error Handling](#error-handling)
8. [Examples](#examples)

---

## Overview

### Base URL

```
http://<host>:8080
```

Default port: `8080` (configurable via `HEALTH_CHECK_PORT` environment variable)

### Content Types

| Endpoint        | Response Content-Type                      |
| --------------- | ------------------------------------------ |
| `/health`       | `application/json`                         |
| `/health/live`  | `application/json`                         |
| `/health/ready` | `application/json`                         |
| `/metrics`      | `text/plain; version=0.0.4; charset=utf-8` |

### HTTP Methods

All endpoints use `GET` method and are read-only.

---

## Health Check Endpoints

### GET /health

Returns comprehensive health status of the connector, including dependency status and SLA metrics.

#### Request

```http
GET /health HTTP/1.1
Host: localhost:8080
Accept: application/json
```

#### Response

**Success (200 OK)** - Connector is healthy or degraded but operational:

```json
{
  "status": "healthy",
  "uptime": 3600,
  "peersConnected": 3,
  "totalPeers": 3,
  "timestamp": "2026-01-23T10:30:00.000Z",
  "nodeId": "connector-1",
  "version": "1.2.0",
  "dependencies": {
    "tigerbeetle": {
      "status": "up",
      "latencyMs": 2
    },
    "evm": {
      "status": "up",
      "latencyMs": 120
    }
  },
  "sla": {
    "packetSuccessRate": 0.9995,
    "settlementSuccessRate": 0.99,
    "p99LatencyMs": 8
  }
}
```

**Degraded (200 OK)** - Some non-critical issues but still operational:

```json
{
  "status": "degraded",
  "uptime": 3600,
  "peersConnected": 2,
  "totalPeers": 3,
  "timestamp": "2026-01-23T10:30:00.000Z",
  "nodeId": "connector-1",
  "version": "1.2.0",
  "dependencies": {
    "tigerbeetle": {
      "status": "up",
      "latencyMs": 2
    },
    "evm": {
      "status": "down"
    }
  },
  "sla": {
    "packetSuccessRate": 0.98,
    "settlementSuccessRate": 0.95,
    "p99LatencyMs": 25
  }
}
```

**Unhealthy (503 Service Unavailable)** - Critical failure:

```json
{
  "status": "unhealthy",
  "uptime": 3600,
  "peersConnected": 0,
  "totalPeers": 3,
  "timestamp": "2026-01-23T10:30:00.000Z",
  "nodeId": "connector-1",
  "version": "1.2.0",
  "dependencies": {
    "tigerbeetle": {
      "status": "down"
    }
  },
  "sla": {
    "packetSuccessRate": 0,
    "settlementSuccessRate": 0,
    "p99LatencyMs": 0
  }
}
```

**Starting (503 Service Unavailable)** - Connector initializing:

```json
{
  "status": "starting",
  "uptime": 5,
  "peersConnected": 0,
  "totalPeers": 3,
  "timestamp": "2026-01-23T10:30:00.000Z",
  "nodeId": "connector-1"
}
```

#### Status Codes

| Code | Meaning                                       |
| ---- | --------------------------------------------- |
| 200  | Healthy or degraded (operational)             |
| 503  | Unhealthy or starting (not ready for traffic) |

#### curl Example

```bash
# Basic health check
curl -s http://localhost:8080/health | jq .

# Check only status
curl -s http://localhost:8080/health | jq -r '.status'

# Exit with error if unhealthy
curl -sf http://localhost:8080/health > /dev/null || echo "Unhealthy!"
```

---

### GET /health/live

Kubernetes liveness probe endpoint. Returns 200 if the process is running.

#### Request

```http
GET /health/live HTTP/1.1
Host: localhost:8080
```

#### Response

**Success (200 OK)**:

```json
{
  "status": "alive",
  "timestamp": "2026-01-23T10:30:00.000Z"
}
```

#### Use Case

Configure as Kubernetes liveness probe to restart crashed containers:

```yaml
livenessProbe:
  httpGet:
    path: /health/live
    port: 8080
  initialDelaySeconds: 10
  periodSeconds: 10
  failureThreshold: 3
```

#### curl Example

```bash
curl -s http://localhost:8080/health/live | jq .
```

---

### GET /health/ready

Kubernetes readiness probe endpoint. Returns 200 only when the connector is ready to handle traffic (dependencies up).

#### Request

```http
GET /health/ready HTTP/1.1
Host: localhost:8080
```

#### Response

**Ready (200 OK)**:

```json
{
  "status": "ready",
  "dependencies": {
    "tigerbeetle": {
      "status": "up",
      "latencyMs": 2
    },
    "evm": {
      "status": "up",
      "latencyMs": 120
    }
  },
  "timestamp": "2026-01-23T10:30:00.000Z"
}
```

**Not Ready (503 Service Unavailable)**:

```json
{
  "status": "not_ready",
  "dependencies": {
    "tigerbeetle": {
      "status": "down"
    }
  },
  "timestamp": "2026-01-23T10:30:00.000Z"
}
```

#### Use Case

Configure as Kubernetes readiness probe to remove from load balancer when not ready:

```yaml
readinessProbe:
  httpGet:
    path: /health/ready
    port: 8080
  initialDelaySeconds: 5
  periodSeconds: 5
  failureThreshold: 2
```

#### curl Example

```bash
# Check readiness
curl -s http://localhost:8080/health/ready | jq .

# Check if ready (exit 0 if ready, non-zero otherwise)
curl -sf http://localhost:8080/health/ready > /dev/null && echo "Ready" || echo "Not ready"
```

---

## Metrics Endpoint

### GET /metrics

Prometheus metrics endpoint exposing connector operational metrics.

#### Request

```http
GET /metrics HTTP/1.1
Host: localhost:8080
Accept: text/plain
```

#### Response

**Success (200 OK)**:

```
# HELP ilp_packets_processed_total Total number of ILP packets processed
# TYPE ilp_packets_processed_total counter
ilp_packets_processed_total{type="prepare",status="success"} 15234
ilp_packets_processed_total{type="prepare",status="error"} 12
ilp_packets_processed_total{type="fulfill",status="success"} 15222
ilp_packets_processed_total{type="reject",status="success"} 12

# HELP ilp_packet_latency_seconds ILP packet processing latency in seconds
# TYPE ilp_packet_latency_seconds histogram
ilp_packet_latency_seconds_bucket{type="prepare",le="0.001"} 5000
ilp_packet_latency_seconds_bucket{type="prepare",le="0.005"} 12000
ilp_packet_latency_seconds_bucket{type="prepare",le="0.01"} 14500
ilp_packet_latency_seconds_bucket{type="prepare",le="0.05"} 15200
ilp_packet_latency_seconds_bucket{type="prepare",le="0.1"} 15234
ilp_packet_latency_seconds_bucket{type="prepare",le="0.5"} 15234
ilp_packet_latency_seconds_bucket{type="prepare",le="1"} 15234
ilp_packet_latency_seconds_bucket{type="prepare",le="+Inf"} 15234
ilp_packet_latency_seconds_sum{type="prepare"} 45.678
ilp_packet_latency_seconds_count{type="prepare"} 15234

# HELP ilp_packets_in_flight Current number of ILP packets being processed
# TYPE ilp_packets_in_flight gauge
ilp_packets_in_flight 5

# HELP settlements_executed_total Total number of settlements executed
# TYPE settlements_executed_total counter
settlements_executed_total{method="evm",status="success"} 150
settlements_executed_total{method="evm",status="failure"} 2

# HELP settlement_latency_seconds Settlement operation latency in seconds
# TYPE settlement_latency_seconds histogram
settlement_latency_seconds_bucket{method="evm",le="1"} 50
settlement_latency_seconds_bucket{method="evm",le="3"} 130
settlement_latency_seconds_bucket{method="evm",le="5"} 145
settlement_latency_seconds_bucket{method="evm",le="10"} 150
settlement_latency_seconds_bucket{method="evm",le="30"} 152
settlement_latency_seconds_bucket{method="evm",le="60"} 152
settlement_latency_seconds_bucket{method="evm",le="+Inf"} 152

# HELP settlement_amount_total Total amount settled
# TYPE settlement_amount_total counter
settlement_amount_total{method="evm",token="USDC"} 1500000000000

# HELP account_balance_units Current account balance in smallest units
# TYPE account_balance_units gauge
account_balance_units{peer_id="peer-1",token_id="USDC"} 500000000
account_balance_units{peer_id="peer-2",token_id="USDC"} 750000000

# HELP payment_channels_active Number of active payment channels
# TYPE payment_channels_active gauge
payment_channels_active{method="evm",status="open"} 3
payment_channels_active{method="evm",status="disputed"} 0

# HELP payment_channels_funded_total Total number of channels funded
# TYPE payment_channels_funded_total counter
payment_channels_funded_total{method="evm"} 5

# HELP payment_channels_disputes_total Total number of channel disputes
# TYPE payment_channels_disputes_total counter
payment_channels_disputes_total{method="evm"} 0

# HELP connector_errors_total Total number of connector errors
# TYPE connector_errors_total counter
connector_errors_total{type="packet_error",severity="warning"} 12
connector_errors_total{type="settlement_error",severity="critical"} 2

# HELP connector_last_error_timestamp Timestamp of the last error
# TYPE connector_last_error_timestamp gauge
connector_last_error_timestamp 1706007000

# Default Node.js metrics (if enabled)
# HELP process_cpu_seconds_total Total user and system CPU time spent in seconds.
# TYPE process_cpu_seconds_total counter
process_cpu_seconds_total 125.45

# HELP process_resident_memory_bytes Resident memory size in bytes.
# TYPE process_resident_memory_bytes gauge
process_resident_memory_bytes 150000000

# HELP nodejs_heap_size_total_bytes Process heap size from Node.js in bytes.
# TYPE nodejs_heap_size_total_bytes gauge
nodejs_heap_size_total_bytes 100000000
```

#### Available Metrics

##### Packet Metrics

| Metric                        | Type      | Labels       | Description                     |
| ----------------------------- | --------- | ------------ | ------------------------------- |
| `ilp_packets_processed_total` | Counter   | type, status | Total packets processed         |
| `ilp_packet_latency_seconds`  | Histogram | type         | Packet processing latency       |
| `ilp_packets_in_flight`       | Gauge     | -            | Current packets being processed |

##### Settlement Metrics

| Metric                       | Type      | Labels         | Description          |
| ---------------------------- | --------- | -------------- | -------------------- |
| `settlements_executed_total` | Counter   | method, status | Total settlements    |
| `settlement_latency_seconds` | Histogram | method         | Settlement latency   |
| `settlement_amount_total`    | Counter   | method, token  | Total amount settled |

##### Account Metrics

| Metric                  | Type    | Labels            | Description     |
| ----------------------- | ------- | ----------------- | --------------- |
| `account_balance_units` | Gauge   | peer_id, token_id | Current balance |
| `account_credits_total` | Counter | peer_id           | Total credits   |
| `account_debits_total`  | Counter | peer_id           | Total debits    |

##### Channel Metrics

| Metric                            | Type    | Labels         | Description      |
| --------------------------------- | ------- | -------------- | ---------------- |
| `payment_channels_active`         | Gauge   | method, status | Active channels  |
| `payment_channels_funded_total`   | Counter | method         | Channels funded  |
| `payment_channels_closed_total`   | Counter | method, reason | Channels closed  |
| `payment_channels_disputes_total` | Counter | method         | Channel disputes |

##### Error Metrics

| Metric                           | Type    | Labels         | Description          |
| -------------------------------- | ------- | -------------- | -------------------- |
| `connector_errors_total`         | Counter | type, severity | Total errors         |
| `connector_last_error_timestamp` | Gauge   | -              | Last error timestamp |

##### Process Metrics (Node.js Default)

| Metric                          | Type    | Description    |
| ------------------------------- | ------- | -------------- |
| `process_cpu_seconds_total`     | Counter | CPU time used  |
| `process_resident_memory_bytes` | Gauge   | Memory usage   |
| `nodejs_heap_size_total_bytes`  | Gauge   | Heap size      |
| `nodejs_eventloop_lag_seconds`  | Gauge   | Event loop lag |

#### Prometheus Configuration

```yaml
# prometheus.yml
scrape_configs:
  - job_name: 'agent-runtime'
    static_configs:
      - targets: ['connector:8080']
    metrics_path: /metrics
    scrape_interval: 10s
```

#### curl Example

```bash
# Get all metrics
curl -s http://localhost:8080/metrics

# Get specific metric
curl -s http://localhost:8080/metrics | grep ilp_packets_processed_total

# Calculate packet error rate
curl -s http://localhost:8080/metrics | grep 'ilp_packets_processed_total{type="prepare"'
```

---

## Response Schemas

### HealthStatus

Basic health status response.

```typescript
interface HealthStatus {
  status: 'healthy' | 'unhealthy' | 'starting';
  uptime: number; // Seconds since start
  peersConnected: number; // Connected peer count
  totalPeers: number; // Configured peer count
  timestamp: string; // ISO 8601 timestamp
  nodeId?: string; // Connector identifier
  version?: string; // Connector version
}
```

### HealthStatusExtended

Extended health status with dependencies and SLA.

```typescript
interface HealthStatusExtended extends HealthStatus {
  status: 'healthy' | 'degraded' | 'unhealthy' | 'starting';
  dependencies: {
    tigerbeetle: DependencyHealthStatus;
    evm?: DependencyHealthStatus;
  };
  sla: SLAMetricsSnapshot;
}

interface DependencyHealthStatus {
  status: 'up' | 'down';
  latencyMs?: number;
}

interface SLAMetricsSnapshot {
  packetSuccessRate: number; // 0.0 - 1.0
  settlementSuccessRate: number; // 0.0 - 1.0
  p99LatencyMs: number; // Milliseconds
}
```

### LivenessResponse

```typescript
interface LivenessResponse {
  status: 'alive';
  timestamp: string;
}
```

### ReadinessResponse

```typescript
interface ReadinessResponse {
  status: 'ready' | 'not_ready';
  dependencies?: {
    tigerbeetle: DependencyHealthStatus;
    evm?: DependencyHealthStatus;
  };
  reason?: string;
  timestamp: string;
}
```

---

## Authentication

### Default Configuration

By default, health and metrics endpoints do **not** require authentication but should be restricted by network access.

### Network Restriction (Recommended)

```bash
# UFW: Allow only internal network
sudo ufw allow from 10.0.0.0/8 to any port 8080
```

### Nginx Basic Auth (Optional)

```nginx
location /metrics {
    auth_basic "Metrics";
    auth_basic_user_file /etc/nginx/.htpasswd;
    proxy_pass http://localhost:8080/metrics;
}
```

### API Key Header (If Implemented)

```http
GET /metrics HTTP/1.1
Host: localhost:8080
X-API-Key: your-api-key
```

---

## Rate Limiting

### Default Limits

Health check endpoints have no rate limiting by default to ensure monitoring reliability.

### Recommended Limits (If Implementing)

| Endpoint        | Limit    | Window   |
| --------------- | -------- | -------- |
| `/health`       | 100 req  | 1 minute |
| `/health/live`  | 1000 req | 1 minute |
| `/health/ready` | 1000 req | 1 minute |
| `/metrics`      | 60 req   | 1 minute |

### Nginx Rate Limiting

```nginx
# Define rate limit zone
limit_req_zone $binary_remote_addr zone=health:10m rate=10r/s;

location /health {
    limit_req zone=health burst=20 nodelay;
    proxy_pass http://localhost:8080/health;
}
```

---

## Error Handling

### Error Response Format

```json
{
  "status": "unhealthy",
  "error": "Failed to retrieve health status"
}
```

### HTTP Status Codes

| Code | Description                                 |
| ---- | ------------------------------------------- |
| 200  | Success (healthy or degraded)               |
| 503  | Service Unavailable (unhealthy or starting) |
| 500  | Internal Server Error (unexpected error)    |

### Common Error Scenarios

| Scenario               | Status Code | Response                                                                       |
| ---------------------- | ----------- | ------------------------------------------------------------------------------ |
| TigerBeetle down       | 503         | `{"status": "unhealthy", "dependencies": {"tigerbeetle": {"status": "down"}}}` |
| All peers disconnected | 503         | `{"status": "unhealthy", "peersConnected": 0}`                                 |
| Connector starting     | 503         | `{"status": "starting"}`                                                       |
| Internal error         | 500         | `{"status": "unhealthy", "error": "..."}`                                      |

---

## Examples

### Docker Health Check

```dockerfile
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
  CMD curl -f http://localhost:8080/health/live || exit 1
```

### Kubernetes Probes

```yaml
apiVersion: apps/v1
kind: Deployment
spec:
  template:
    spec:
      containers:
        - name: connector
          ports:
            - containerPort: 8080
              name: health
          livenessProbe:
            httpGet:
              path: /health/live
              port: health
            initialDelaySeconds: 10
            periodSeconds: 10
            failureThreshold: 3
          readinessProbe:
            httpGet:
              path: /health/ready
              port: health
            initialDelaySeconds: 5
            periodSeconds: 5
            failureThreshold: 2
          startupProbe:
            httpGet:
              path: /health/ready
              port: health
            initialDelaySeconds: 5
            periodSeconds: 5
            failureThreshold: 30 # 2.5 minutes to start
```

### Monitoring Scripts

```bash
#!/bin/bash
# health-monitor.sh - Monitor connector health

CONNECTOR_URL="${1:-http://localhost:8080}"

# Check health status
health=$(curl -sf "$CONNECTOR_URL/health" 2>/dev/null)
status=$(echo "$health" | jq -r '.status')

case "$status" in
  "healthy")
    echo "✓ Connector is healthy"
    exit 0
    ;;
  "degraded")
    echo "⚠ Connector is degraded"
    echo "$health" | jq '.dependencies'
    exit 1
    ;;
  "unhealthy"|"starting")
    echo "✗ Connector is $status"
    echo "$health" | jq .
    exit 2
    ;;
  *)
    echo "✗ Failed to get health status"
    exit 3
    ;;
esac
```

### Prometheus Alerting

```yaml
groups:
  - name: connector-api
    rules:
      - alert: HealthEndpointDown
        expr: up{job="agent-runtime"} == 0
        for: 1m
        labels:
          severity: critical
        annotations:
          summary: 'Connector health endpoint unreachable'

      - alert: ConnectorUnhealthy
        expr: probe_success{job="agent-runtime-health"} == 0
        for: 2m
        labels:
          severity: critical
        annotations:
          summary: 'Connector reporting unhealthy status'
```

### Grafana Dashboard Queries

```promql
# Connector status (1 = healthy, 0 = unhealthy)
probe_success{job="agent-runtime-health"}

# Packet throughput
rate(ilp_packets_processed_total[5m])

# Error rate percentage
sum(rate(ilp_packets_processed_total{status="error"}[5m])) / sum(rate(ilp_packets_processed_total[5m])) * 100

# P99 latency
histogram_quantile(0.99, rate(ilp_packet_latency_seconds_bucket[5m]))

# Settlement success rate
sum(rate(settlements_executed_total{status="success"}[5m])) / sum(rate(settlements_executed_total[5m])) * 100

# Active payment channels
sum(payment_channels_active{status="open"})
```

---

## Support Resources

- **Monitoring Setup**: [monitoring-setup-guide.md](./monitoring-setup-guide.md)
- **Incident Response**: [incident-response-runbook.md](./incident-response-runbook.md)
- **Production Deployment**: [production-deployment-guide.md](./production-deployment-guide.md)

---

**Document Version**: 1.0
**Last Updated**: 2026-01-23
**Author**: Dev Agent James (Story 12.9)
