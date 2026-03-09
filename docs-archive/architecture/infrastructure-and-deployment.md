# Infrastructure and Deployment

## Infrastructure as Code

- **Tool:** Docker Compose 2.24.x
- **Location:** `docker/docker-compose.yml` (and topology variants)
- **Approach:** Declarative container orchestration with environment-based configuration

**Decision Rationale:**

- Docker Compose sufficient for MVP (single-machine deployment)
- YAML format aligns with topology configuration files
- No Terraform/Pulumi needed (no cloud resources)
- Future migration to Kubernetes possible if cloud deployment needed

## Deployment Strategy

- **Strategy:** Local container deployment with manual execution
- **CI/CD Platform:** GitHub Actions
- **Pipeline Configuration:** `.github/workflows/ci.yml`

**Deployment Flow:**

1. Developer runs `docker-compose up` locally
2. Docker pulls pre-built images (if published) or builds from Dockerfiles
3. Containers start with health checks (connectors, agent wallet, TigerBeetle)
4. Health checks accessible at `http://localhost:8080/health` per container

**CI/CD Pipeline Stages:**

1. **Build:** Compile TypeScript for all packages
2. **Lint:** Run ESLint and Prettier checks
3. **Test:** Execute Jest unit and integration tests
4. **Docker Build:** Build connector and service images (on main branch)
5. **Optional:** Push images to GitHub Container Registry

## Environments

- **Local Development:** Primary environment - `docker-compose up` on developer machine
  - All services run on localhost (connectors, agent wallet, TigerBeetle)
  - Direct log access via `docker-compose logs`
  - Structured JSON logging to stdout for monitoring

- **CI/CD Testing:** GitHub Actions runners
  - Automated test execution
  - Docker build validation
  - No persistent state between runs

- **Future Production (Post-MVP):** Cloud deployment with Kubernetes
  - Multi-node connector clusters
  - Persistent TigerBeetle cluster
  - Log aggregation and monitoring dashboards

## Environment Promotion Flow

```
Local Development
  ↓ (git push)
GitHub Actions CI
  ↓ (tests pass)
Docker Image Build
  ↓ (manual tag/release)
GitHub Container Registry
  ↓ (future: automated deployment)
Cloud Environment (Kubernetes)
```

**MVP Scope:** Promotion stops at Docker Image Build. Cloud deployment deferred to post-MVP.

## Rollback Strategy

- **Primary Method:** Container restart with previous image tag
- **Trigger Conditions:**
  - Health checks failing after deployment
  - Critical bugs discovered in new version
  - Performance degradation beyond NFR thresholds
- **Recovery Time Objective:** < 2 minutes (restart containers with previous image)

**Rollback Procedure:**

```bash

```
