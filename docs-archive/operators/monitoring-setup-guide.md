# ILP Connector Monitoring Setup Guide

This guide covers the setup and configuration of production monitoring infrastructure for the ILP Connector, including Prometheus metrics collection, Grafana dashboards, alerting, distributed tracing, and log aggregation.

## Table of Contents

- [Architecture Overview](#architecture-overview)
- [Quick Start](#quick-start)
- [Prometheus Setup](#prometheus-setup)
- [Grafana Setup](#grafana-setup)
- [Alertmanager Configuration](#alertmanager-configuration)
- [OpenTelemetry Tracing](#opentelemetry-tracing)
- [Log Aggregation](#log-aggregation)
- [Kubernetes Deployment](#kubernetes-deployment)
- [Configuration Reference](#configuration-reference)
- [Troubleshooting](#troubleshooting)

---

## Architecture Overview

```
┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
│  ILP Connector  │────▶│   Prometheus    │────▶│    Grafana      │
│   :8080/metrics │     │     :9090       │     │     :3000       │
└─────────────────┘     └────────┬────────┘     └─────────────────┘
        │                        │
        │                        ▼
        │               ┌─────────────────┐
        │               │  Alertmanager   │
        │               │     :9093       │
        │               └─────────────────┘
        │
        │ (traces)
        ▼
┌─────────────────┐     ┌─────────────────┐
│  OTLP Collector │────▶│  Jaeger/Tempo   │
│     :4318       │     │     :16686      │
└─────────────────┘     └─────────────────┘
        │
        │ (logs)
        ▼
┌─────────────────┐
│  ELK/Datadog/   │
│   CloudWatch    │
└─────────────────┘
```

### Components

| Component      | Purpose                        | Default Port |
| -------------- | ------------------------------ | ------------ |
| ILP Connector  | Application metrics and health | 8080         |
| Prometheus     | Metrics collection and storage | 9090         |
| Grafana        | Metrics visualization          | 3000         |
| Alertmanager   | Alert routing and notification | 9093         |
| Jaeger/Tempo   | Distributed trace storage      | 16686        |
| OTLP Collector | OpenTelemetry trace collection | 4318         |

---

## Quick Start

### Using Docker Compose

The fastest way to get started with monitoring is using the provided Docker Compose configuration:

```bash
# Start the monitoring stack alongside the connector
docker-compose -f docker-compose-dev.yml -f docker-compose-monitoring.yml up -d

# Verify services are running
docker-compose -f docker-compose-dev.yml -f docker-compose-monitoring.yml ps
```

### Accessing Dashboards

Once running, access the monitoring interfaces:

- **Grafana:** http://localhost:3000 (default: admin/admin)
- **Prometheus:** http://localhost:9090
- **Alertmanager:** http://localhost:9093
- **Jaeger UI:** http://localhost:16686

### Verify Metrics Collection

```bash
# Check connector metrics
curl http://localhost:8080/metrics

# Check Prometheus targets
curl http://localhost:9090/api/v1/targets | jq '.data.activeTargets'

# Check connector health
curl http://localhost:8080/health | jq
```

---

## Prometheus Setup

### Installation

#### Docker

```yaml
# docker-compose-monitoring.yml
services:
  prometheus:
    image: prom/prometheus:v2.47.0
    ports:
      - '9090:9090'
    volumes:
      - ./monitoring/prometheus/prometheus.yml:/etc/prometheus/prometheus.yml
      - ./monitoring/prometheus/alerts:/etc/prometheus/alerts
      - prometheus-data:/prometheus
    command:
      - '--config.file=/etc/prometheus/prometheus.yml'
      - '--storage.tsdb.path=/prometheus'
      - '--web.enable-lifecycle'
    restart: unless-stopped

volumes:
  prometheus-data:
```

#### Kubernetes

```yaml
# prometheus-deployment.yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: prometheus
spec:
  replicas: 1
  selector:
    matchLabels:
      app: prometheus
  template:
    spec:
      containers:
        - name: prometheus
          image: prom/prometheus:v2.47.0
          ports:
            - containerPort: 9090
          volumeMounts:
            - name: config
              mountPath: /etc/prometheus
            - name: storage
              mountPath: /prometheus
      volumes:
        - name: config
          configMap:
            name: prometheus-config
        - name: storage
          persistentVolumeClaim:
            claimName: prometheus-pvc
```

### Configuration

The Prometheus configuration file (`monitoring/prometheus/prometheus.yml`) defines scrape targets:

```yaml
global:
  scrape_interval: 15s
  evaluation_interval: 15s

rule_files:
  - /etc/prometheus/alerts/*.yml

scrape_configs:
  # ILP Connector metrics
  - job_name: 'agent-runtime'
    static_configs:
      - targets: ['connector:8080']
    metrics_path: /metrics
    scrape_interval: 10s

  # TigerBeetle metrics (if exposed)
  - job_name: 'tigerbeetle'
    static_configs:
      - targets: ['tigerbeetle:3000']
    scrape_interval: 30s

  # Prometheus self-monitoring
  - job_name: 'prometheus'
    static_configs:
      - targets: ['localhost:9090']
```

### Service Discovery (Kubernetes)

For Kubernetes deployments, use service discovery:

```yaml
scrape_configs:
  - job_name: 'kubernetes-pods'
    kubernetes_sd_configs:
      - role: pod
    relabel_configs:
      - source_labels: [__meta_kubernetes_pod_annotation_prometheus_io_scrape]
        action: keep
        regex: true
      - source_labels: [__meta_kubernetes_pod_annotation_prometheus_io_path]
        action: replace
        target_label: __metrics_path__
        regex: (.+)
```

Add pod annotations for auto-discovery:

```yaml
metadata:
  annotations:
    prometheus.io/scrape: 'true'
    prometheus.io/port: '8080'
    prometheus.io/path: '/metrics'
```

---

## Grafana Setup

### Installation

#### Docker

```yaml
# docker-compose-monitoring.yml
services:
  grafana:
    image: grafana/grafana:10.2.0
    ports:
      - '3000:3000'
    environment:
      - GF_SECURITY_ADMIN_PASSWORD=admin
      - GF_USERS_ALLOW_SIGN_UP=false
    volumes:
      - ./monitoring/grafana/provisioning:/etc/grafana/provisioning
      - ./monitoring/grafana/dashboards:/var/lib/grafana/dashboards
      - grafana-data:/var/lib/grafana
    restart: unless-stopped

volumes:
  grafana-data:
```

### Dashboard Provisioning

Dashboards are automatically provisioned from the `monitoring/grafana/dashboards/` directory.

#### Datasource Configuration

Create `monitoring/grafana/provisioning/datasources/datasources.yml`:

```yaml
apiVersion: 1

datasources:
  - name: Prometheus
    type: prometheus
    access: proxy
    url: http://prometheus:9090
    isDefault: true
    editable: false

  - name: Jaeger
    type: jaeger
    access: proxy
    url: http://jaeger:16686
    editable: false
```

#### Dashboard Provisioning Configuration

Create `monitoring/grafana/provisioning/dashboards/dashboards.yml`:

```yaml
apiVersion: 1

providers:
  - name: 'ILP Connector Dashboards'
    orgId: 1
    folder: 'ILP Connector'
    type: file
    disableDeletion: false
    editable: true
    options:
      path: /var/lib/grafana/dashboards
```

### Available Dashboards

| Dashboard           | Description                                 | File                     |
| ------------------- | ------------------------------------------- | ------------------------ |
| Connector Overview  | Network-wide packet and settlement metrics  | connector-overview.json  |
| Connector Health    | Resource utilization and dependency status  | connector-health.json    |
| Settlement Activity | Settlement operations and channel lifecycle | settlement-activity.json |

### Importing Dashboards Manually

If not using provisioning, import dashboards via the Grafana UI:

1. Navigate to Dashboards > Import
2. Upload the JSON file or paste the JSON content
3. Select the Prometheus datasource
4. Click Import

---

## Alertmanager Configuration

### Installation

```yaml
# docker-compose-monitoring.yml
services:
  alertmanager:
    image: prom/alertmanager:v0.26.0
    ports:
      - '9093:9093'
    volumes:
      - ./monitoring/alertmanager/alertmanager.yml:/etc/alertmanager/alertmanager.yml
    command:
      - '--config.file=/etc/alertmanager/alertmanager.yml'
    restart: unless-stopped
```

### Configuration

Create `monitoring/alertmanager/alertmanager.yml`:

```yaml
global:
  resolve_timeout: 5m
  slack_api_url: 'https://hooks.slack.com/services/YOUR/SLACK/WEBHOOK'

route:
  group_by: ['alertname', 'severity']
  group_wait: 10s
  group_interval: 10s
  repeat_interval: 1h
  receiver: 'slack-notifications'
  routes:
    - match:
        severity: critical
      receiver: 'pagerduty-critical'
    - match:
        severity: warning
      receiver: 'slack-notifications'

receivers:
  - name: 'slack-notifications'
    slack_configs:
      - channel: '#agent-runtime-alerts'
        send_resolved: true
        title: '{{ .Status | toUpper }}: {{ .CommonAnnotations.summary }}'
        text: '{{ .CommonAnnotations.description }}'

  - name: 'pagerduty-critical'
    pagerduty_configs:
      - service_key: 'YOUR_PAGERDUTY_SERVICE_KEY'
        severity: '{{ .CommonLabels.severity }}'
        description: '{{ .CommonAnnotations.summary }}'

inhibit_rules:
  - source_match:
      severity: 'critical'
    target_match:
      severity: 'warning'
    equal: ['alertname']
```

### Alert Rules

Alert rules are defined in `monitoring/prometheus/alerts/connector-alerts.yml`. The following alerts are configured:

| Alert                  | Severity | Description                              |
| ---------------------- | -------- | ---------------------------------------- |
| HighPacketErrorRate    | warning  | Error rate > 5% for 2 minutes            |
| SettlementFailures     | critical | Any settlement failure for 1 minute      |
| TigerBeetleUnavailable | critical | TigerBeetle down for 1 minute            |
| ChannelDispute         | high     | Any payment channel in disputed state    |
| HighP99Latency         | warning  | P99 latency > 10ms for 5 minutes         |
| LowThroughput          | warning  | Throughput < 1000 TPS for 5 minutes      |
| ConnectorDown          | critical | Health endpoint unreachable for 1 minute |
| HighMemoryUsage        | warning  | Memory usage > 85% for 5 minutes         |
| CriticalErrorSpike     | critical | > 10 critical errors/minute for 1 minute |
| SettlementSLABreach    | warning  | Settlement success < 99% for 5 minutes   |
| PacketSLABreach        | warning  | Packet success < 99.9% for 5 minutes     |

---

## OpenTelemetry Tracing

### Connector Configuration

Enable tracing in the connector configuration:

```yaml
# connector-config.yaml
observability:
  opentelemetry:
    enabled: true
    serviceName: 'agent-runtime'
    exporterEndpoint: 'http://jaeger:4318/v1/traces'
    samplingRatio: 1.0 # 100% sampling for dev, reduce in production
```

Or via environment variables:

```bash
OTEL_ENABLED=true
OTEL_SERVICE_NAME=agent-runtime
OTEL_EXPORTER_ENDPOINT=http://jaeger:4318/v1/traces
OTEL_SAMPLING_RATIO=1.0
```

### Jaeger Setup

```yaml
# docker-compose-monitoring.yml
services:
  jaeger:
    image: jaegertracing/all-in-one:1.51
    ports:
      - '16686:16686' # Jaeger UI
      - '4318:4318' # OTLP HTTP
    environment:
      - COLLECTOR_OTLP_ENABLED=true
    restart: unless-stopped
```

### Grafana Tempo (Alternative)

For production, consider Grafana Tempo for scalable trace storage:

```yaml
services:
  tempo:
    image: grafana/tempo:2.3.0
    ports:
      - '4318:4318'
    volumes:
      - ./monitoring/tempo/tempo.yaml:/etc/tempo.yaml
    command: ['-config.file=/etc/tempo.yaml']
```

### Trace Attributes

The following span attributes are added to traces:

| Attribute           | Description                     |
| ------------------- | ------------------------------- |
| `ilp.destination`   | ILP packet destination address  |
| `ilp.amount`        | Packet amount in smallest units |
| `peer.source`       | Source peer ID                  |
| `peer.destination`  | Destination peer ID             |
| `settlement.method` | Settlement method (evm)         |
| `settlement.amount` | Settlement amount               |
| `error.type`        | Error type if applicable        |

---

## Log Aggregation

### Structured Logging

The ILP Connector outputs structured JSON logs to stdout, compatible with all major log aggregation platforms.

Example log entry:

```json
{
  "level": "info",
  "time": 1703620800000,
  "nodeId": "connector-a",
  "correlationId": "pkt_abc123def456",
  "component": "packet-handler",
  "msg": "Packet forwarded successfully",
  "destination": "g.example.user",
  "amount": 1000000,
  "latencyMs": 2.5
}
```

### ELK Stack Integration

#### Filebeat Configuration

```yaml
# filebeat.yml
filebeat.inputs:
  - type: container
    paths:
      - '/var/lib/docker/containers/*/*.log'
    processors:
      - decode_json_fields:
          fields: ['message']
          target: ''
          overwrite_keys: true

output.elasticsearch:
  hosts: ['elasticsearch:9200']
  index: 'agent-runtime-%{+yyyy.MM.dd}'
```

#### Logstash Configuration

```ruby
# logstash.conf
input {
  beats {
    port => 5044
  }
}

filter {
  json {
    source => "message"
  }
  date {
    match => ["time", "UNIX_MS"]
    target => "@timestamp"
  }
}

output {
  elasticsearch {
    hosts => ["elasticsearch:9200"]
    index => "agent-runtime-%{+YYYY.MM.dd}"
  }
}
```

### Datadog Integration

```yaml
# docker-compose-monitoring.yml
services:
  datadog-agent:
    image: datadog/agent:7
    environment:
      - DD_API_KEY=${DD_API_KEY}
      - DD_LOGS_ENABLED=true
      - DD_LOGS_CONFIG_CONTAINER_COLLECT_ALL=true
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock
      - /proc/:/host/proc/:ro
      - /sys/fs/cgroup/:/host/sys/fs/cgroup:ro
```

Add labels to connector container:

```yaml
services:
  connector:
    labels:
      com.datadoghq.ad.logs: '[{"source": "agent-runtime", "service": "agent-runtime"}]'
```

### AWS CloudWatch Integration

```yaml
# docker-compose-monitoring.yml
services:
  cloudwatch-agent:
    image: amazon/cloudwatch-agent:latest
    environment:
      - AWS_REGION=${AWS_REGION}
    volumes:
      - ./monitoring/cloudwatch/config.json:/etc/cwagentconfig
      - /var/run/docker.sock:/var/run/docker.sock
```

CloudWatch agent configuration:

```json
{
  "logs": {
    "logs_collected": {
      "files": {
        "collect_list": [
          {
            "file_path": "/var/log/containers/agent-runtime*.log",
            "log_group_name": "/agent-runtime/logs",
            "log_stream_name": "{instance_id}",
            "timezone": "UTC"
          }
        ]
      }
    }
  }
}
```

---

## Kubernetes Deployment

### Complete Monitoring Stack

```yaml
# monitoring-namespace.yaml
apiVersion: v1
kind: Namespace
metadata:
  name: monitoring
---
# prometheus-configmap.yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: prometheus-config
  namespace: monitoring
data:
  prometheus.yml: |
    global:
      scrape_interval: 15s
    scrape_configs:
      - job_name: 'kubernetes-pods'
        kubernetes_sd_configs:
          - role: pod
        relabel_configs:
          - source_labels: [__meta_kubernetes_pod_annotation_prometheus_io_scrape]
            action: keep
            regex: true
---
# grafana-deployment.yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: grafana
  namespace: monitoring
spec:
  replicas: 1
  selector:
    matchLabels:
      app: grafana
  template:
    metadata:
      labels:
        app: grafana
    spec:
      containers:
        - name: grafana
          image: grafana/grafana:10.2.0
          ports:
            - containerPort: 3000
          volumeMounts:
            - name: dashboards
              mountPath: /var/lib/grafana/dashboards
            - name: provisioning
              mountPath: /etc/grafana/provisioning
      volumes:
        - name: dashboards
          configMap:
            name: grafana-dashboards
        - name: provisioning
          configMap:
            name: grafana-provisioning
```

### Helm Chart (Recommended)

For production Kubernetes deployments, use the kube-prometheus-stack Helm chart:

```bash
helm repo add prometheus-community https://prometheus-community.github.io/helm-charts
helm repo update

helm install monitoring prometheus-community/kube-prometheus-stack \
  --namespace monitoring \
  --create-namespace \
  --set grafana.adminPassword=your-secure-password \
  -f monitoring-values.yaml
```

---

## Configuration Reference

### Connector Observability Configuration

```yaml
# Full observability configuration reference
observability:
  prometheus:
    enabled: true # Enable Prometheus metrics
    metricsPath: '/metrics' # Metrics endpoint path
    includeDefaultMetrics: true # Include Node.js default metrics
    labels: # Global labels for all metrics
      environment: 'production'
      nodeId: 'connector-1'

  opentelemetry:
    enabled: true # Enable distributed tracing
    serviceName: 'agent-runtime' # Service name in traces
    exporterEndpoint: 'http://jaeger:4318/v1/traces' # OTLP endpoint
    samplingRatio: 0.1 # Sample 10% of traces

  sla:
    packetSuccessRateThreshold: 0.999 # 99.9% packet success target
    settlementSuccessRateThreshold: 0.99 # 99% settlement success target
    p99LatencyThresholdMs: 10 # 10ms p99 latency target
```

### Environment Variables

| Variable                           | Description                  | Default                 |
| ---------------------------------- | ---------------------------- | ----------------------- |
| `PROMETHEUS_ENABLED`               | Enable Prometheus metrics    | `true`                  |
| `PROMETHEUS_METRICS_PATH`          | Metrics endpoint path        | `/metrics`              |
| `OTEL_ENABLED`                     | Enable OpenTelemetry tracing | `false`                 |
| `OTEL_SERVICE_NAME`                | Service name for traces      | `connector`             |
| `OTEL_EXPORTER_ENDPOINT`           | OTLP exporter endpoint       | `http://localhost:4318` |
| `OTEL_SAMPLING_RATIO`              | Trace sampling ratio         | `1.0`                   |
| `SLA_PACKET_SUCCESS_THRESHOLD`     | Packet success rate SLA      | `0.999`                 |
| `SLA_SETTLEMENT_SUCCESS_THRESHOLD` | Settlement success rate SLA  | `0.99`                  |
| `SLA_P99_LATENCY_MS`               | P99 latency SLA in ms        | `10`                    |

---

## Troubleshooting

### Common Issues

| Issue                     | Symptoms               | Cause                       | Solution                                   |
| ------------------------- | ---------------------- | --------------------------- | ------------------------------------------ |
| No metrics in Prometheus  | Targets show "down"    | Network connectivity        | Check Docker network, firewall rules       |
| No metrics in Prometheus  | Connection refused     | Metrics endpoint disabled   | Set `PROMETHEUS_ENABLED=true`              |
| Dashboard empty           | No data points         | Wrong datasource URL        | Verify Prometheus URL in datasource        |
| Dashboard empty           | Query errors           | Dashboard version mismatch  | Import latest dashboard JSON               |
| No traces in Jaeger       | Services not appearing | OTLP endpoint misconfigured | Verify `OTEL_EXPORTER_OTLP_ENDPOINT`       |
| No traces in Jaeger       | No spans visible       | Tracing disabled            | Set `OTEL_ENABLED=true`                    |
| Alerts not firing         | Rules show "inactive"  | Expression never matches    | Test PromQL query manually                 |
| Alerts not firing         | No notifications       | Alertmanager unreachable    | Check Prometheus → Alertmanager connection |
| High cardinality warnings | Prometheus slow        | Too many label combinations | Reduce metric labels                       |
| Disk space issues         | Prometheus OOM         | Retention too long          | Reduce `--storage.tsdb.retention.time`     |

### Prometheus Not Scraping Metrics

1. **Check target status:**

   ```bash
   curl http://prometheus:9090/api/v1/targets | jq '.data.activeTargets[] | {instance, health}'
   ```

2. **Verify connector metrics endpoint:**

   ```bash
   curl http://connector:8080/metrics
   ```

3. **Check network connectivity:**
   ```bash
   docker exec prometheus wget -qO- http://connector:8080/metrics
   ```

### Grafana Dashboard Not Loading

1. **Check datasource configuration:**
   - Navigate to Configuration > Data Sources
   - Test the Prometheus connection

2. **Verify dashboard JSON:**

   ```bash
   jq . monitoring/grafana/dashboards/connector-overview.json
   ```

3. **Check Grafana logs:**
   ```bash
   docker logs grafana --tail 100
   ```

### Traces Not Appearing in Jaeger

1. **Verify OTLP endpoint:**

   ```bash
   curl -X POST http://jaeger:4318/v1/traces -H "Content-Type: application/json" -d '{}'
   ```

2. **Check connector tracing configuration:**

   ```bash
   curl http://connector:8080/health | jq '.config.observability.opentelemetry'
   ```

3. **Review connector logs for tracing errors:**
   ```bash
   docker logs connector 2>&1 | grep -i 'otel\|trace\|span'
   ```

### Alerts Not Firing

1. **Check Prometheus alert status:**

   ```bash
   curl http://prometheus:9090/api/v1/rules | jq '.data.groups[].rules[] | {name, state}'
   ```

2. **Verify Alertmanager connectivity:**

   ```bash
   curl http://prometheus:9090/api/v1/alertmanagers | jq
   ```

3. **Test alert expression:**
   ```bash
   curl 'http://prometheus:9090/api/v1/query?query=rate(connector_errors_total[5m])' | jq
   ```

---

## Related Documentation

| Document                                                        | Description                         |
| --------------------------------------------------------------- | ----------------------------------- |
| [Production Deployment Guide](./production-deployment-guide.md) | Initial deployment setup            |
| [Incident Response Runbook](./incident-response-runbook.md)     | Handling incidents and alerts       |
| [API Reference](./api-reference.md)                             | Health and metrics endpoint details |
| [Performance Tuning Guide](./performance-tuning-guide.md)       | Optimization and tuning             |

## External Resources

- [Prometheus Documentation](https://prometheus.io/docs/)
- [Grafana Documentation](https://grafana.com/docs/)
- [OpenTelemetry Documentation](https://opentelemetry.io/docs/)
- [Jaeger Documentation](https://www.jaegertracing.io/docs/)

---

**Document Version**: 1.1
**Last Updated**: 2026-01-23
