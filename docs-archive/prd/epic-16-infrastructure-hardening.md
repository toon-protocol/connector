# Epic 16: Infrastructure Hardening & CI/CD Improvements

## Brownfield Enhancement

This epic addresses findings from an infrastructure review to improve build reliability, security posture, and production readiness of the M2M ILP Connector. Changes span CI/CD configuration, Docker build processes, security hardening, and monitoring/alerting infrastructure.

---

## Epic Goal

Remediate critical and high-priority infrastructure findings to ensure reliable builds across all target platforms, enforce security best practices in CI/CD pipelines, and establish production-grade alerting and resource management for deployment environments.

---

## Epic Description

### Existing System Context

- **Current relevant functionality:** The project has a mature CI/CD pipeline (GitHub Actions), Docker-based deployment (13 compose files), Prometheus/Grafana monitoring stack, and multi-cloud KMS support for key management.
- **Technology stack:** GitHub Actions, Docker/Docker Compose, Node.js 22.x, Prometheus, Grafana, TigerBeetle, semantic-release
- **Integration points:**
  - `.github/workflows/ci.yml` - Main CI pipeline with lint, test, build, security, and deploy jobs
  - `Dockerfile` - Multi-stage production image build
  - `docker-compose-production.yml` - Production deployment stack
  - `monitoring/prometheus/` - Metrics collection and alerting configuration
  - `.env.example` - Environment configuration template

### Enhancement Details

**What's being added/changed:**

1. **Node Version Alignment:** Fix critical mismatch between Dockerfile (Node 20) and package.json requirement (Node ≥22.11.0), ensuring consistent runtime across development, CI, and production.

2. **Multi-Architecture Docker Builds:** Extend Docker image builds to support both `linux/amd64` and `linux/arm64` platforms, enabling deployment on ARM-based infrastructure (AWS Graviton, Apple Silicon development).

3. **Security Pipeline Hardening:**
   - Make `npm audit` failures blocking (currently `continue-on-error: true`)
   - Enforce security scan results in CI gate decisions
   - Document and enforce KMS-only key management for production deployments

4. **Production Secrets Management:**
   - Replace default Grafana password with Docker secrets or mandatory environment override
   - Add validation to prevent deployment with default credentials
   - Document secrets rotation procedures

5. **Alertmanager Configuration:**
   - Enable and configure Alertmanager for production notifications
   - Add notification channels (Slack, PagerDuty, email templates)
   - Create escalation policies for critical alerts

6. **Resource Limits & Production Hardening:**
   - Add CPU/memory resource limits to production Docker Compose
   - Document TigerBeetle high-availability multi-replica configuration
   - Fix service dependency conditions (`service_healthy` vs `service_started`)

**How it integrates:**

All changes are configuration-level updates to existing infrastructure files. No application code changes required. The enhancements improve reliability and security without altering connector functionality.

**Success criteria:**

- CI pipeline fails on security vulnerabilities (npm audit, Snyk)
- Docker images build successfully for both amd64 and arm64
- Node version consistent across Dockerfile, .nvmrc, and CI workflows
- Production compose files include resource limits
- Alertmanager sends notifications for critical alerts
- Default credentials cannot be used in production deployments

---

## Stories

### Story 16.1: Node Version Alignment & Multi-Architecture Docker Builds

**Goal:** Fix the Node version mismatch and enable multi-platform Docker builds for ARM64 support.

**Scope:**

- Update `Dockerfile` to use `node:22-alpine` instead of `node:20-alpine` in all stages (builder, ui-builder, runtime)
- Update `.github/workflows/ci.yml` docker-build-push job to build for multiple platforms:
  ```yaml
  platforms: linux/amd64,linux/arm64
  ```
- Verify `.nvmrc` matches package.json engine requirement (22.11.0)
- Update any other Dockerfiles (Dockerfile.dev, Dockerfile.client-ui) for consistency
- Test Docker build on both platforms via GitHub Actions

**Acceptance Criteria:**

1. [ ] `Dockerfile` uses `node:22-alpine` base image in all stages
2. [ ] `Dockerfile.dev` uses `node:22-alpine` base image
3. [ ] `Dockerfile.client-ui` uses `node:22-alpine` base image (if exists)
4. [ ] CI docker-build-push job specifies `platforms: linux/amd64,linux/arm64`
5. [ ] Docker image builds successfully on CI for both architectures
6. [ ] `.nvmrc` contains `22.11.0` matching package.json engines
7. [ ] Local `docker build` completes without Node version warnings

**Technical Notes:**

- Node 22 is LTS as of October 2024
- Multi-arch builds require `docker/setup-qemu-action` for ARM emulation on x86 runners
- Build time will increase ~50-100% due to dual-platform compilation
- Consider using build matrix for parallel platform builds if time-sensitive

---

### Story 16.2: Security Pipeline Hardening

**Goal:** Ensure security vulnerabilities block CI pipeline and enforce secure practices for production deployments.

**Scope:**

- Update `.github/workflows/ci.yml` security job:
  - Remove `continue-on-error: true` from npm audit step
  - Configure npm audit to fail on `high` or `critical` vulnerabilities
  - Ensure Snyk scan failures block the pipeline (remove `continue-on-error`)
- Add CI job dependency so `ci-status` fails if security job fails
- Update `.env.example` with stronger warnings about `KEY_BACKEND=env`
- Add production deployment preflight check script that validates:
  - `KEY_BACKEND` is not `env`
  - `GRAFANA_PASSWORD` is not `admin`
  - Required KMS credentials are set
- Document security requirements in operator guide

**Acceptance Criteria:**

1. [ ] npm audit with high/critical vulnerabilities fails CI pipeline
2. [ ] Snyk scan failures fail CI pipeline (not continue-on-error)
3. [ ] `ci-status` job includes security job in failure check
4. [ ] `scripts/production-preflight.sh` validates KEY_BACKEND is not 'env'
5. [ ] `scripts/production-preflight.sh` validates GRAFANA_PASSWORD is not 'admin'
6. [ ] `.env.example` includes prominent warning about dev-only settings
7. [ ] `docs/operators/security-hardening-guide.md` updated with CI security requirements

**Technical Notes:**

- npm audit exit codes: 0 = no vulnerabilities, non-zero = vulnerabilities found
- Use `npm audit --audit-level=high` to only fail on high/critical
- Snyk action has `args: --severity-threshold=high` already configured
- Preflight script should exit non-zero with clear error messages
- Consider adding `--production` flag to npm audit to skip devDependencies

---

### Story 16.3: Production Alerting & Resource Management

**Goal:** Enable Alertmanager for production notifications and add resource limits to production deployments.

**Scope:**

- Create `monitoring/alertmanager/alertmanager.yml` configuration:
  - Global settings (resolve timeout, SMTP config template)
  - Route tree for alert routing by severity
  - Receiver templates for Slack, email, PagerDuty (placeholder configs)
  - Inhibition rules to suppress duplicate alerts
- Update `docker-compose-production.yml`:
  - Add Alertmanager service definition
  - Uncomment Alertmanager target in `prometheus.yml`
  - Add `deploy.resources.limits` to all services:
    - connector: 1 CPU, 1GB memory
    - tigerbeetle: 0.5 CPU, 512MB memory
    - prometheus: 0.5 CPU, 512MB memory
    - grafana: 0.25 CPU, 256MB memory
  - Change `connector-b` dependency to `service_healthy` (if applicable)
- Update `docker-compose-monitoring.yml` to include Alertmanager
- Create `docs/operators/alerting-setup-guide.md` with:
  - Alertmanager configuration instructions
  - Slack/PagerDuty integration steps
  - Alert routing customization
  - Testing alert delivery

**Acceptance Criteria:**

1. [ ] `monitoring/alertmanager/alertmanager.yml` created with valid configuration
2. [ ] Alertmanager service added to `docker-compose-production.yml`
3. [ ] Prometheus `alertmanager` target uncommented and functional
4. [ ] All production services have CPU and memory limits defined
5. [ ] `docker-compose up` with production file starts Alertmanager successfully
6. [ ] Test alert (manual trigger) routes to configured receiver
7. [ ] `docs/operators/alerting-setup-guide.md` documents configuration steps
8. [ ] Service dependencies use `service_healthy` condition where health checks exist

**Technical Notes:**

- Alertmanager default port: 9093
- Resource limits use Docker Compose deploy syntax (requires `docker-compose --compatibility` or Swarm mode)
- For non-Swarm deployments, use `mem_limit` and `cpus` directly
- TigerBeetle may need higher limits under load - document tuning
- Prometheus alert rules already reference Alertmanager in config (currently commented)

---

## Compatibility Requirements

- [x] Existing APIs remain unchanged (infrastructure-only changes)
- [x] Database schema changes are backward compatible (N/A - no DB changes)
- [x] CI pipeline changes are backward compatible (stricter enforcement only)
- [x] Docker Compose files remain compatible with existing workflows
- [x] Monitoring dashboards unaffected (same metrics, new alerting)

---

## Risk Mitigation

- **Primary Risk:** CI pipeline starts failing due to previously-ignored vulnerabilities
- **Mitigation:**
  - Audit current vulnerabilities before enabling blocking mode
  - Create issues for existing vulnerabilities that need remediation
  - Use `npm audit fix` to auto-fix where possible
  - Document any accepted risks with `npm audit --omit=dev` exclusions
- **Secondary Risk:** Multi-arch builds significantly slow CI
- **Mitigation:**
  - Use GitHub Actions cache for Docker layers
  - Consider matrix builds for parallel platform compilation
  - Monitor build times and optimize if >15 minutes
- **Rollback Plan:**
  - Revert to `continue-on-error: true` if critical vulnerabilities cannot be immediately fixed
  - Revert to single-platform builds if ARM64 issues arise

---

## Definition of Done

- [ ] All stories completed with acceptance criteria met
- [ ] Node version consistent across all Dockerfiles and CI
- [ ] Docker images available for both amd64 and arm64 on GHCR
- [ ] Security vulnerabilities block CI pipeline
- [ ] Production preflight script validates secure configuration
- [ ] Alertmanager operational with test notification verified
- [ ] Resource limits defined for all production services
- [ ] Documentation updated for operators
- [ ] No regression in existing CI/CD functionality

---

## Technical Implementation Notes

### Node Version Matrix

| File                   | Current        | Target         | Notes          |
| ---------------------- | -------------- | -------------- | -------------- |
| `package.json` engines | ≥22.11.0       | ≥22.11.0       | No change      |
| `.nvmrc`               | 22.11.0        | 22.11.0        | Verify matches |
| `Dockerfile`           | node:20-alpine | node:22-alpine | **UPDATE**     |
| `Dockerfile.dev`       | node:20-alpine | node:22-alpine | **UPDATE**     |
| CI workflow            | 22.11.0, 22.x  | 22.11.0, 22.x  | No change      |

### Multi-Arch Build Configuration

```yaml
# .github/workflows/ci.yml - docker-build-push job
- name: Set up QEMU
  uses: docker/setup-qemu-action@v3

- name: Set up Docker Buildx
  uses: docker/setup-buildx-action@v3

- name: Build and push Docker image
  uses: docker/build-push-action@v5
  with:
    context: .
    push: true
    platforms: linux/amd64,linux/arm64
    tags: ${{ steps.meta.outputs.tags }}
    cache-from: type=gha
    cache-to: type=gha,mode=max
```

### Alertmanager Configuration Template

```yaml
# monitoring/alertmanager/alertmanager.yml
global:
  resolve_timeout: 5m

route:
  group_by: ['alertname', 'severity']
  group_wait: 30s
  group_interval: 5m
  repeat_interval: 4h
  receiver: 'default'
  routes:
    - match:
        severity: critical
      receiver: 'critical-alerts'
    - match:
        severity: high
      receiver: 'high-alerts'

receivers:
  - name: 'default'
    # Configure webhook, email, or slack
  - name: 'critical-alerts'
    # PagerDuty or immediate notification
  - name: 'high-alerts'
    # Slack channel notification

inhibit_rules:
  - source_match:
      severity: 'critical'
    target_match:
      severity: 'warning'
    equal: ['alertname']
```

### Resource Limits Reference

```yaml
# docker-compose-production.yml service limits
services:
  connector:
    deploy:
      resources:
        limits:
          cpus: '1.0'
          memory: 1G
        reservations:
          cpus: '0.5'
          memory: 512M
```

---

## Future Enhancements (Out of Scope)

- Kubernetes manifests with NetworkPolicy and HPA
- Move Prover CI integration with Boogie/Z3
- Persistent Jaeger storage for production tracing
- TigerBeetle multi-replica HA cluster deployment
- OIDC/SSO integration for Grafana
- Automated security patch workflows (Dependabot auto-merge)
