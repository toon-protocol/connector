# Operator Documentation

This directory contains comprehensive documentation for deploying, operating, and maintaining the M2M ILP Connector in production environments.

## Quick Start

New to the M2M Connector? Follow these guides in order:

1. **[Production Deployment Guide](./production-deployment-guide.md)** - Initial setup and deployment
2. **[Peer Onboarding Guide](./peer-onboarding-guide.md)** - Join the network
3. **[Monitoring Setup Guide](./monitoring-setup-guide.md)** - Configure monitoring
4. **[Security Hardening Guide](./security-hardening-guide.md)** - Secure your deployment

## Documentation Index

### Deployment & Configuration

| Document                                                        | Description                                 | Audience    |
| --------------------------------------------------------------- | ------------------------------------------- | ----------- |
| [Production Deployment Guide](./production-deployment-guide.md) | Step-by-step deployment with Docker Compose | DevOps, SRE |
| [Peer Onboarding Guide](./peer-onboarding-guide.md)             | Joining the M2M network, peer discovery     | Operators   |
| [Performance Tuning Guide](./performance-tuning-guide.md)       | Optimization for 10K+ TPS                   | DevOps, SRE |

### Security

| Document                                                              | Description                                 | Audience         |
| --------------------------------------------------------------------- | ------------------------------------------- | ---------------- |
| [Security Hardening Guide](./security-hardening-guide.md)             | TLS, KMS, network security, audit checklist | Security, DevOps |
| [Production Readiness Checklist](./production-readiness-checklist.md) | Pre-deployment verification                 | All teams        |

### Operations & Maintenance

| Document                                                      | Description                      | Audience     |
| ------------------------------------------------------------- | -------------------------------- | ------------ |
| [Monitoring Setup Guide](./monitoring-setup-guide.md)         | Prometheus, Grafana, alerting    | DevOps, SRE  |
| [Incident Response Runbook](./incident-response-runbook.md)   | Handling incidents and outages   | On-call, SRE |
| [Backup and Disaster Recovery](./backup-disaster-recovery.md) | Backup procedures, DR strategies | DevOps, SRE  |
| [Upgrade and Migration Guide](./upgrade-migration-guide.md)   | Version upgrades, rollbacks      | DevOps       |
| [Wallet Backup Recovery](./wallet-backup-recovery.md)         | Agent wallet backup and restore  | Operators    |

### Reference

| Document                            | Description                          | Audience           |
| ----------------------------------- | ------------------------------------ | ------------------ |
| [API Reference](./api-reference.md) | Health, metrics, and admin endpoints | Developers, DevOps |

## Document Matrix by Task

| Task                 | Primary Document                                          | Related Documents                    |
| -------------------- | --------------------------------------------------------- | ------------------------------------ |
| Deploy new connector | [Production Deployment](./production-deployment-guide.md) | Security Hardening, Monitoring Setup |
| Join network as peer | [Peer Onboarding](./peer-onboarding-guide.md)             | Production Deployment                |
| Set up monitoring    | [Monitoring Setup](./monitoring-setup-guide.md)           | API Reference, Incident Response     |
| Handle incident      | [Incident Response](./incident-response-runbook.md)       | Monitoring Setup, API Reference      |
| Upgrade connector    | [Upgrade Migration](./upgrade-migration-guide.md)         | Backup DR, Production Deployment     |
| Restore from backup  | [Backup DR](./backup-disaster-recovery.md)                | Wallet Backup, Incident Response     |
| Security audit       | [Security Hardening](./security-hardening-guide.md)       | Production Readiness Checklist       |
| Optimize performance | [Performance Tuning](./performance-tuning-guide.md)       | Monitoring Setup                     |
| Configure APIs       | [API Reference](./api-reference.md)                       | Monitoring Setup                     |

## Quick Reference

### Common Commands

```bash
# Start connector stack
docker compose -f docker-compose-production.yml up -d

# Check health
curl http://localhost:8080/health | jq .

# View logs
docker compose -f docker-compose-production.yml logs -f connector

# Check metrics
curl http://localhost:8080/metrics

# Manual rollback
IMAGE_NAME=your-org/agent-runtime ./scripts/rollback.sh v1.2.0

# Create backup
./scripts/backup.sh --full --verify --upload
```

### Key Ports

| Port | Service       | Purpose                |
| ---- | ------------- | ---------------------- |
| 4000 | BTP WebSocket | Peer connections       |
| 8080 | HTTP          | Health checks, metrics |
| 9090 | Prometheus    | Metrics database       |
| 3001 | Grafana       | Dashboards             |
| 9093 | Alertmanager  | Alert management       |

### Environment Variables

| Variable             | Required | Description                      |
| -------------------- | -------- | -------------------------------- |
| `NODE_ID`            | Yes      | Unique connector identifier      |
| `KEY_BACKEND`        | Yes      | `aws-kms`, `gcp-kms`, `azure-kv` |
| `BASE_RPC_URL`       | Yes      | Base L2 RPC endpoint             |
| `PROMETHEUS_ENABLED` | No       | Enable metrics (default: true)   |
| `OTEL_ENABLED`       | No       | Enable tracing (default: false)  |

### Health Status Codes

| Status      | HTTP Code | Meaning                    |
| ----------- | --------- | -------------------------- |
| `healthy`   | 200       | All systems operational    |
| `degraded`  | 200       | Some non-critical issues   |
| `unhealthy` | 503       | Critical failure           |
| `starting`  | 503       | Initialization in progress |

## Troubleshooting Quick Links

- **Service won't start** → [Production Deployment: Troubleshooting](./production-deployment-guide.md#troubleshooting)
- **Health check failing** → [API Reference: Health Endpoints](./api-reference.md#health-check-endpoints)
- **No metrics in Grafana** → [Monitoring Setup: Troubleshooting](./monitoring-setup-guide.md#troubleshooting)
- **Peer connection issues** → [Peer Onboarding: Common Issues](./peer-onboarding-guide.md#common-issues)
- **Performance issues** → [Performance Tuning: Troubleshooting](./performance-tuning-guide.md#troubleshooting)
- **Security incident** → [Incident Response: Security Scenarios](./incident-response-runbook.md)

## Contributing

When updating documentation:

1. Follow the existing document structure
2. Include "Common Issues" tables in troubleshooting sections
3. Add cross-references to related documents
4. Update this index when adding new documents
5. Include version and last updated date at document end

## Support

- **Issues**: https://github.com/m2m-network/m2m/issues
- **Documentation**: This directory
- **Security Issues**: security@m2m.network

---

**Documentation Version**: 1.0
**Last Updated**: 2026-01-23
