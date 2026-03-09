# Alerting Setup Guide

**Table of Contents**

- [Introduction](#introduction)
- [Alertmanager Configuration](#alertmanager-configuration)
- [Slack Integration](#slack-integration)
- [PagerDuty Integration](#pagerduty-integration)
- [Email Notifications](#email-notifications)
- [Alert Routing Customization](#alert-routing-customization)
- [Testing Alert Delivery](#testing-alert-delivery)
- [Silencing Alerts](#silencing-alerts)
- [Troubleshooting](#troubleshooting)
- [Reference](#reference)
- [Future Enhancements](#future-enhancements)

---

## Introduction

Alertmanager is the central notification routing system in the M2M ILP Connector monitoring stack. It receives alerts from Prometheus and dispatches notifications to configured channels based on severity and routing rules.

### Alert Flow

```
Prometheus Metrics → Alert Rules → Alertmanager → Notification Channels
                                          ↓
                            (Slack, PagerDuty, Email, etc.)
```

### Alert Severity Levels

The M2M ILP Connector uses three severity levels for alert classification:

- **critical**: System-wide impact requiring immediate attention
  - Example: `ConnectorDown`, `TigerBeetleUnavailable`, `SettlementSLABreach`
  - Routing: Immediate notification via PagerDuty or critical channels
  - Repeat interval: Every 1 hour until resolved

- **high**: Service degradation requiring urgent attention
  - Example: `HighPacketErrorRate`, `SettlementFailures`, `ChannelDispute`
  - Routing: Urgent notification via Slack or email
  - Repeat interval: Every 4 hours until resolved

- **warning**: Potential issues requiring monitoring
  - Example: `HighMemoryUsage`, `LowThroughput`, `HighP99Latency`
  - Routing: Standard notification channels
  - Repeat interval: Every 4 hours until resolved

All alert rules are defined in `monitoring/prometheus/alerts/connector-alerts.yml`.

---

## Alertmanager Configuration

### Configuration File Location

Alertmanager configuration is stored in:

```
monitoring/alertmanager/alertmanager.yml
```

This file is mounted read-only into the Alertmanager container and defines:

- Global settings (SMTP, Slack API, timeouts)
- Route tree (how alerts are grouped and routed)
- Receivers (notification channel configurations)
- Inhibition rules (suppress redundant alerts)

### Configuration Structure

The configuration file has four main sections:

1. **global**: Shared settings for all receivers
   - `resolve_timeout`: Auto-resolve time for alerts (default: 5m)
   - SMTP configuration (for email receivers)
   - Slack API URL (can be set globally or per-receiver)

2. **route**: Alert routing tree
   - `group_by`: Labels used to group alerts (default: `['alertname', 'severity']`)
   - `group_wait`: Delay before first notification (default: 30s)
   - `group_interval`: Delay before sending new alerts in group (default: 5m)
   - `repeat_interval`: Resend frequency for unresolved alerts (default: 4h)
   - `receiver`: Default receiver name
   - `routes`: Severity-based routing rules (critical → critical-alerts, high → high-alerts)

3. **receivers**: Notification channel definitions
   - `default`: Standard notifications (webhook, email, Slack)
   - `critical-alerts`: Immediate response (PagerDuty, critical Slack channel)
   - `high-alerts`: Urgent notifications (Slack, email)

4. **inhibit_rules**: Suppress lower-severity alerts when higher-severity alert active
   - Prevents alert noise during incidents
   - Example: Suppress warning when critical firing for same alertname

### Default Configuration

The default `alertmanager.yml` includes **placeholder configurations** with commented examples for common integrations. Production deployments must customize receivers based on their notification preferences.

### Reloading Configuration

After modifying `alertmanager.yml`, reload the configuration without restarting the container:

```bash
# Hot-reload Alertmanager configuration
docker-compose exec alertmanager kill -HUP 1

# Or restart the container
docker-compose restart alertmanager
```

Verify configuration is valid before reloading:

```bash
docker-compose exec alertmanager amtool check-config /etc/alertmanager/alertmanager.yml
```

---

## Slack Integration

### Step 1: Create Slack Incoming Webhook

1. Navigate to your Slack workspace: https://api.slack.com/messaging/webhooks
2. Click **Create New App** → **From scratch**
3. Name your app (e.g., "ILP Connector Alertmanager") and select workspace
4. Navigate to **Incoming Webhooks** → Enable **Activate Incoming Webhooks**
5. Click **Add New Webhook to Workspace**
6. Select channel for alerts (e.g., `#ilp-alerts`)
7. Copy the Webhook URL (format: `https://hooks.slack.com/services/T00000000/B00000000/XXXXXXXXXXXXXXXXXXXX`)

### Step 2: Configure Slack Receiver

Edit `monitoring/alertmanager/alertmanager.yml` and uncomment/configure a Slack receiver:

```yaml
receivers:
  - name: 'high-alerts'
    slack_configs:
      - api_url: 'https://hooks.slack.com/services/YOUR/WEBHOOK/URL'
        channel: '#ilp-alerts'
        title: '⚠️ HIGH: {{ .GroupLabels.alertname }}'
        text: |
          *Instance:* {{ .GroupLabels.instance }}
          *Summary:* {{ range .Alerts }}{{ .Annotations.summary }}{{ end }}
          *Description:* {{ range .Alerts }}{{ .Annotations.description }}{{ end }}
          *Runbook:* {{ range .Alerts }}{{ .Annotations.runbook_url }}{{ end }}
        color: '{{ if eq .Status "firing" }}danger{{ else }}good{{ end }}'
```

### Step 3: Customize Message Templates

Alertmanager supports Go templating for message content. Useful template variables:

- `{{ .GroupLabels.alertname }}`: Alert name (e.g., "HighPacketErrorRate")
- `{{ .GroupLabels.severity }}`: Alert severity (critical, high, warning)
- `{{ .GroupLabels.instance }}`: Instance identifier (e.g., "agent-runtime")
- `{{ .Status }}`: Alert status ("firing" or "resolved")
- `{{ range .Alerts }}...{{ end }}`: Iterate over all alerts in group
- `{{ .Annotations.summary }}`: Alert summary text
- `{{ .Annotations.description }}`: Detailed alert description
- `{{ .Annotations.runbook_url }}`: Link to incident response runbook

**Color by severity:**

```yaml
color: |
  {{ if eq .GroupLabels.severity "critical" }}danger
  {{ else if eq .GroupLabels.severity "high" }}warning
  {{ else }}good{{ end }}
```

### Step 4: Test Slack Notifications

1. Reload Alertmanager configuration:

   ```bash
   docker-compose exec alertmanager kill -HUP 1
   ```

2. Trigger test alert:

   ```bash
   docker-compose exec alertmanager amtool alert add \
     alertname="SlackTestAlert" severity="high" instance="test" \
     summary="Test Slack notification from Alertmanager"
   ```

3. Verify notification appears in Slack channel within 30 seconds (group_wait duration)

4. Silence the test alert:
   ```bash
   docker-compose exec alertmanager amtool silence add \
     alertname="SlackTestAlert" --duration=1h --comment="Test complete"
   ```

---

## PagerDuty Integration

### Step 1: Create PagerDuty Integration

1. Log in to PagerDuty: https://www.pagerduty.com
2. Navigate to **Services** → Select service or create new service
3. Click **Integrations** tab → **Add Integration**
4. Select **Events API V2** integration type
5. Name the integration (e.g., "ILP Connector Alertmanager")
6. Copy the **Integration Key** (format: `xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx`)

### Step 2: Configure PagerDuty Receiver

Edit `monitoring/alertmanager/alertmanager.yml` and configure PagerDuty for critical alerts:

```yaml
receivers:
  - name: 'critical-alerts'
    pagerduty_configs:
      - service_key: 'YOUR_PAGERDUTY_INTEGRATION_KEY'
        description: '{{ .GroupLabels.alertname }}: {{ .GroupLabels.instance }}'
        details:
          severity: '{{ .GroupLabels.severity }}'
          summary: '{{ range .Alerts }}{{ .Annotations.summary }}{{ end }}'
          description: '{{ range .Alerts }}{{ .Annotations.description }}{{ end }}'
          runbook_url: '{{ range .Alerts }}{{ .Annotations.runbook_url }}{{ end }}'
```

### Step 3: Configure Escalation Policies

In PagerDuty:

1. Navigate to **People** → **Escalation Policies**
2. Create or edit escalation policy for critical ILP alerts
3. Define escalation levels (e.g., on-call engineer → team lead → engineering manager)
4. Set escalation timeout (e.g., 5 minutes between levels)
5. Associate escalation policy with your service

### Step 4: Test PagerDuty Integration

1. Reload Alertmanager configuration:

   ```bash
   docker-compose exec alertmanager kill -HUP 1
   ```

2. Trigger critical test alert:

   ```bash
   docker-compose exec alertmanager amtool alert add \
     alertname="PagerDutyTestAlert" severity="critical" instance="test" \
     summary="Test PagerDuty integration from Alertmanager"
   ```

3. Verify incident created in PagerDuty and on-call engineer notified

4. Acknowledge and resolve incident in PagerDuty UI

---

## Email Notifications

### Step 1: Configure SMTP Settings

Edit `monitoring/alertmanager/alertmanager.yml` global section:

```yaml
global:
  resolve_timeout: 5m

  # SMTP Configuration
  smtp_smarthost: 'smtp.example.com:587'
  smtp_from: 'alertmanager@example.com'
  smtp_auth_username: 'alertmanager'
  smtp_auth_password: 'your-smtp-password'
  smtp_require_tls: true
```

**Common SMTP Providers:**

- **Gmail**: `smtp.gmail.com:587` (requires app-specific password)
- **SendGrid**: `smtp.sendgrid.net:587` (API key as password)
- **AWS SES**: `email-smtp.us-east-1.amazonaws.com:587` (SMTP credentials)
- **Mailgun**: `smtp.mailgun.org:587` (SMTP credentials)

### Step 2: Configure Email Receiver

```yaml
receivers:
  - name: 'default'
    email_configs:
      - to: 'ops-team@example.com'
        subject: 'ILP Connector Alert: {{ .GroupLabels.alertname }}'
        html: |
          <h2>{{ .GroupLabels.alertname }}</h2>
          <p><strong>Severity:</strong> {{ .GroupLabels.severity }}</p>
          <p><strong>Instance:</strong> {{ .GroupLabels.instance }}</p>
          <p><strong>Summary:</strong> {{ range .Alerts }}{{ .Annotations.summary }}{{ end }}</p>
          <p><strong>Description:</strong> {{ range .Alerts }}{{ .Annotations.description }}{{ end }}</p>
          <p><strong>Runbook:</strong> <a href="{{ range .Alerts }}{{ .Annotations.runbook_url }}{{ end }}">View Runbook</a></p>
        headers:
          From: 'ILP Connector Alertmanager <alertmanager@example.com>'
          Reply-To: 'ops-team@example.com'
```

### Step 3: Customize Email Templates

**HTML Template with Severity Colors:**

```yaml
html: |
  <div style="font-family: Arial, sans-serif;">
    <div style="background-color: {{ if eq .GroupLabels.severity "critical" }}#d32f2f{{ else if eq .GroupLabels.severity "high" }}#f57c00{{ else }}#fbc02d{{ end }}; color: white; padding: 15px; margin-bottom: 20px;">
      <h2 style="margin: 0;">{{ .GroupLabels.alertname }}</h2>
      <p style="margin: 5px 0 0 0;">Severity: {{ .GroupLabels.severity | toUpper }}</p>
    </div>
    <div style="padding: 15px;">
      <p><strong>Instance:</strong> {{ .GroupLabels.instance }}</p>
      <p><strong>Status:</strong> {{ .Status }}</p>
      <p><strong>Summary:</strong> {{ range .Alerts }}{{ .Annotations.summary }}{{ end }}</p>
      <p><strong>Description:</strong> {{ range .Alerts }}{{ .Annotations.description }}{{ end }}</p>
      <p><a href="{{ range .Alerts }}{{ .Annotations.runbook_url }}{{ end }}" style="background-color: #1976d2; color: white; padding: 10px 20px; text-decoration: none; display: inline-block; margin-top: 10px;">View Runbook</a></p>
    </div>
  </div>
```

### Step 4: Test Email Notifications

1. Reload Alertmanager and trigger test alert (see Testing Alert Delivery section)
2. Verify email received at configured address
3. Check spam folder if email not received
4. Verify SMTP authentication and TLS configuration if delivery fails

---

## Alert Routing Customization

### Understanding Route Tree

Alertmanager uses a hierarchical route tree to match alerts and determine receivers. Routes are evaluated top-to-bottom; first match wins.

**Default route tree:**

```yaml
route:
  receiver: 'default' # Catch-all for unmatched alerts
  routes:
    - match:
        severity: critical
      receiver: 'critical-alerts'
    - match:
        severity: high
      receiver: 'high-alerts'
```

### Grouping Alerts

**Group by alertname and severity:**

```yaml
route:
  group_by: ['alertname', 'severity']
```

All alerts with same alertname and severity are batched into single notification.

**Group by instance:**

```yaml
route:
  group_by: ['instance']
```

All alerts from same instance (e.g., connector-a) are batched together.

**Disable grouping:**

```yaml
route:
  group_by: []
```

Each alert triggers separate notification (higher notification volume).

### Timing Configuration

**group_wait**: Wait before sending first notification for new group

```yaml
group_wait: 30s # Wait 30s to batch related alerts
```

**group_interval**: Wait before sending new alerts in existing group

```yaml
group_interval: 5m # Send batch every 5m if new alerts arrive
```

**repeat_interval**: Resend unresolved alert notifications

```yaml
repeat_interval: 4h # Remind every 4h until resolved
```

### Route Specific Alerts

**Route SettlementFailures to dedicated receiver:**

```yaml
routes:
  - match:
      alertname: SettlementFailures
    receiver: 'settlement-team'
    repeat_interval: 1h
```

**Route all connector-a alerts to team-a:**

```yaml
routes:
  - match:
      instance: connector-a
    receiver: 'team-a-slack'
```

**Route by multiple labels:**

```yaml
routes:
  - match:
      alertname: HighPacketErrorRate
      severity: critical
    receiver: 'critical-routing-alerts'
```

### Override Timing for Specific Routes

```yaml
routes:
  - match:
      severity: critical
    receiver: 'critical-alerts'
    group_wait: 10s # Send critical alerts faster
    repeat_interval: 1h # Remind every hour
  - match:
      severity: warning
    receiver: 'default'
    repeat_interval: 12h # Remind less frequently for warnings
```

---

## Testing Alert Delivery

### Manual Alert Trigger with amtool

Alertmanager includes `amtool` CLI for testing and management.

**Trigger test alert:**

```bash
docker-compose exec alertmanager amtool alert add \
  alertname="TestAlert" severity="high" instance="test-instance" \
  summary="This is a test alert for Story 16.3 verification"
```

**Verify alert in Alertmanager UI:**

```bash
# Open Alertmanager UI
open http://localhost:9093/#/alerts

# Or check via API
curl http://localhost:9093/api/v2/alerts | jq '.[] | {alertname: .labels.alertname, status: .status.state}'
```

### Verify Alert Routing

**Check which receiver will handle an alert:**

```bash
docker-compose exec alertmanager amtool config routes test \
  alertname=HighPacketErrorRate severity=high instance=connector
```

Expected output shows matched route and receiver.

### Test All Notification Channels

1. **Test default receiver (warning alerts):**

   ```bash
   docker-compose exec alertmanager amtool alert add \
     alertname="TestWarning" severity="warning" instance="test" \
     summary="Test warning notification"
   ```

2. **Test high-alerts receiver:**

   ```bash
   docker-compose exec alertmanager amtool alert add \
     alertname="TestHigh" severity="high" instance="test" \
     summary="Test high-priority notification"
   ```

3. **Test critical-alerts receiver:**

   ```bash
   docker-compose exec alertmanager amtool alert add \
     alertname="TestCritical" severity="critical" instance="test" \
     summary="Test critical notification"
   ```

4. Wait for `group_wait` duration (default: 30s) for notifications to send

5. Verify notification delivery in Slack, PagerDuty, email, etc.

### View Alertmanager Logs

```bash
# Real-time logs
docker-compose logs -f alertmanager

# Last 50 lines
docker-compose logs --tail=50 alertmanager
```

Look for log entries indicating notification dispatch:

```
level=info msg="Notify successful" receiver=high-alerts
```

---

## Silencing Alerts

Silences suppress alert notifications during maintenance windows or known issues.

### Create Silence via CLI

**Silence specific alert by name:**

```bash
docker-compose exec alertmanager amtool silence add \
  alertname="HighMemoryUsage" \
  --duration=2h \
  --comment="Maintenance window: database migration"
```

**Silence all alerts for instance:**

```bash
docker-compose exec alertmanager amtool silence add \
  instance="connector-a" \
  --duration=4h \
  --comment="Scheduled connector upgrade"
```

**Silence by multiple labels:**

```bash
docker-compose exec alertmanager amtool silence add \
  alertname="SettlementFailures" severity="high" \
  --duration=1h \
  --comment="Scheduled maintenance"
```

### Create Silence via UI

1. Open Alertmanager UI: http://localhost:9093
2. Click **Silences** tab
3. Click **New Silence** button
4. Add matchers (label: value pairs)
5. Set duration and expiration
6. Add comment describing reason
7. Click **Create** button

### List Active Silences

```bash
# List all silences
docker-compose exec alertmanager amtool silence query

# List silences matching specific alert
docker-compose exec alertmanager amtool silence query alertname=HighMemoryUsage
```

### Expire Silence Early

```bash
# Get silence ID from list command
docker-compose exec alertmanager amtool silence query

# Expire silence by ID
docker-compose exec alertmanager amtool silence expire <SILENCE_ID>
```

### Silence Best Practices

- **Always add meaningful comments**: Explain why silence was created
- **Use shortest necessary duration**: Don't silence longer than needed
- **Test alert delivery after**: Verify alerts resume when silence expires
- **Document planned silences**: Coordinate with team before silencing critical alerts

---

## Troubleshooting

### Alert Not Firing

**Check Prometheus alert rule evaluation:**

```bash
# View all alert rules
curl http://localhost:9090/api/v1/rules | jq '.data.groups[].rules[] | {alert: .name, state: .state}'

# Check specific alert
curl http://localhost:9090/api/v1/rules | jq '.data.groups[].rules[] | select(.name=="HighPacketErrorRate")'
```

**Verify alert condition is met:**

- Open Prometheus UI: http://localhost:9090/alerts
- Check if alert shows as "Pending" or "Firing"
- Review alert query in Prometheus Graph view

**Check Prometheus logs:**

```bash
docker-compose logs prometheus | grep -i error
```

### Alert Firing but No Notification

**Verify Alertmanager receiving alerts:**

```bash
# Check Alertmanager alerts
curl http://localhost:9093/api/v2/alerts | jq '.[] | {alertname: .labels.alertname, status: .status.state}'
```

**Check Prometheus → Alertmanager connection:**

```bash
# View Alertmanager targets in Prometheus
curl http://localhost:9090/api/v1/targets | jq '.data.activeTargets[] | select(.job=="alertmanager")'
```

Expected: `"health": "up"`

**Verify routing configuration:**

```bash
# Test which receiver will handle alert
docker-compose exec alertmanager amtool config routes test \
  alertname=YourAlertName severity=high instance=connector

# Show full routing tree
docker-compose exec alertmanager amtool config routes
```

**Check if alert is silenced:**

```bash
docker-compose exec alertmanager amtool silence query alertname=YourAlertName
```

### Notification Not Received

**For Slack:**

- Verify webhook URL is correct (test with curl)
- Check Slack app permissions
- Review Alertmanager logs for "Notify failed" errors
- Test webhook manually:
  ```bash
  curl -X POST -H 'Content-type: application/json' \
    --data '{"text":"Test notification"}' \
    https://hooks.slack.com/services/YOUR/WEBHOOK/URL
  ```

**For PagerDuty:**

- Verify integration key is correct
- Check PagerDuty service is active
- Review escalation policy assignment
- Test integration in PagerDuty UI

**For Email:**

- Verify SMTP credentials and authentication
- Check SMTP server allows connections from Alertmanager IP
- Review spam/junk folder
- Test SMTP connection:
  ```bash
  docker-compose exec alertmanager sh -c "echo 'Test email' | mail -s 'Test' recipient@example.com"
  ```

### View Alertmanager Logs

```bash
# Real-time logs
docker-compose logs -f alertmanager

# Filter for errors
docker-compose logs alertmanager | grep -i error

# Filter for specific receiver
docker-compose logs alertmanager | grep "receiver=high-alerts"
```

### Validate Configuration

**Validate Alertmanager config syntax:**

```bash
docker-compose exec alertmanager amtool check-config /etc/alertmanager/alertmanager.yml
```

Expected output: `Config is valid`

**Validate Prometheus config:**

```bash
docker-compose exec prometheus promtool check config /etc/prometheus/prometheus.yml
```

Expected output: `SUCCESS`

**Validate Prometheus alert rules:**

```bash
docker-compose exec prometheus promtool check rules /etc/prometheus/alerts/connector-alerts.yml
```

### Common Issues

**Issue: Alerts not grouping correctly**

- Check `group_by` labels in route configuration
- Verify alerts have expected labels
- Review grouping in Alertmanager UI

**Issue: Too many notifications (alert spam)**

- Increase `group_interval` and `repeat_interval`
- Add inhibition rules to suppress related alerts
- Use silences for known issues

**Issue: Notifications delayed**

- Reduce `group_wait` duration for faster notifications
- Check Alertmanager processing performance
- Review network latency to notification endpoints

---

## Reference

### Official Documentation

- **Alertmanager Documentation**: https://prometheus.io/docs/alerting/latest/alertmanager/
- **Alertmanager Configuration**: https://prometheus.io/docs/alerting/latest/configuration/
- **amtool Command Reference**: https://github.com/prometheus/alertmanager#amtool

### Internal Documentation

- **Alert Rules**: `monitoring/prometheus/alerts/connector-alerts.yml`
- **Incident Response Runbook**: [docs/operators/incident-response-runbook.md](incident-response-runbook.md)
- **Security Hardening Guide**: [docs/operators/security-hardening-guide.md](security-hardening-guide.md)

### Service Endpoints

- **Alertmanager UI**: http://localhost:9093
- **Prometheus Alerts**: http://localhost:9090/alerts
- **Grafana Dashboards**: http://localhost:3001

### Configuration Files

- **Alertmanager Config**: `monitoring/alertmanager/alertmanager.yml`
- **Prometheus Config**: `monitoring/prometheus/prometheus.yml`
- **Alert Rules**: `monitoring/prometheus/alerts/connector-alerts.yml`
- **Docker Compose Production**: `docker-compose-production.yml`
- **Docker Compose Monitoring**: `docker-compose-monitoring.yml`

---

## Future Enhancements

### TigerBeetle High Availability

TigerBeetle supports high-availability multi-replica clusters (3+ replicas) for production resilience and fault tolerance. The current production deployment uses a single-replica configuration optimized for development and testing.

For production deployments requiring high availability, TigerBeetle can be configured in a multi-replica cluster with quorum-based consensus. This provides automatic failover, data replication, and continued operation during node failures.

**Note:** Detailed multi-replica cluster setup, replica coordination, and production HA configuration are beyond the scope of this deployment guide. Operators interested in HA deployment should refer to the official TigerBeetle documentation: https://docs.tigerbeetle.com/

Multi-replica TigerBeetle configuration will be addressed in a future infrastructure hardening epic.
