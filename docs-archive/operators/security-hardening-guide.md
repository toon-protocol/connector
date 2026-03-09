# Security Hardening Guide

This guide provides comprehensive security hardening procedures for production M2M ILP Connector deployments, including network security, TLS configuration, key management, authentication, and security audit checklists.

## Table of Contents

1. [Security Overview](#security-overview)
2. [CI/CD Security Pipeline](#cicd-security-pipeline)
3. [Network Security](#network-security)
4. [TLS/HTTPS Configuration](#tlshttps-configuration)
5. [Key Management (HSM/KMS)](#key-management-hsmkms)
6. [Secrets Management](#secrets-management)
7. [Authentication and Authorization](#authentication-and-authorization)
8. [Audit Logging](#audit-logging)
9. [Security Monitoring and Alerting](#security-monitoring-and-alerting)
10. [Container Security](#container-security)
11. [Production Security Audit Checklist](#production-security-audit-checklist)

---

## Security Overview

### Defense in Depth

The M2M Connector employs multiple layers of security:

```
┌─────────────────────────────────────────────────────────────────┐
│                     External Network                             │
├─────────────────────────────────────────────────────────────────┤
│  ┌──────────────┐                                               │
│  │   Firewall   │ ◄── Layer 1: Network perimeter                │
│  └──────┬───────┘                                               │
│         │                                                        │
│  ┌──────▼───────┐                                               │
│  │   TLS/SSL    │ ◄── Layer 2: Transport encryption             │
│  └──────┬───────┘                                               │
│         │                                                        │
│  ┌──────▼───────┐                                               │
│  │    Auth      │ ◄── Layer 3: Authentication                   │
│  └──────┬───────┘                                               │
│         │                                                        │
│  ┌──────▼───────┐                                               │
│  │   HSM/KMS    │ ◄── Layer 4: Key protection                   │
│  └──────┬───────┘                                               │
│         │                                                        │
│  ┌──────▼───────┐                                               │
│  │ Audit Logs   │ ◄── Layer 5: Monitoring & detection           │
│  └─────────────┘                                                │
└─────────────────────────────────────────────────────────────────┘
```

### Security Principles

1. **Least Privilege**: Grant minimum permissions required
2. **Defense in Depth**: Multiple security layers
3. **Fail Secure**: Default to deny access
4. **Audit Everything**: Log all security-relevant events
5. **Encrypt at Rest and Transit**: Protect data everywhere

---

## CI/CD Security Pipeline

**Story 16.2: Security Pipeline Hardening**

The M2M project enforces security best practices through automated CI/CD gates that block vulnerable code from reaching production. This section documents the security scanning infrastructure, vulnerability response procedures, and production deployment validation.

### Automated Security Scanning

The CI pipeline includes blocking security scans that prevent merging code with known vulnerabilities:

```yaml
# .github/workflows/ci.yml (Security Job)
security:
  name: Security Audit
  runs-on: ubuntu-latest
  steps:
    - name: Run npm audit
      run: npm audit --audit-level=high
      # BLOCKING: Fails CI on high/critical vulnerabilities

    - name: Run Snyk security scan
      uses: snyk/actions/node@master
      env:
        SNYK_TOKEN: ${{ secrets.SNYK_TOKEN }}
      with:
        args: --severity-threshold=high
      # BLOCKING: Fails CI on high/critical findings
```

**Key Configuration:**

| Tool      | Severity Threshold          | Behavior                     | Scope                |
| --------- | --------------------------- | ---------------------------- | -------------------- |
| npm audit | `--audit-level=high`        | Blocks on high/critical CVEs | Production deps only |
| Snyk      | `--severity-threshold=high` | Blocks on high/critical      | All dependencies     |

**CI Status Gate:**

The `ci-status` job validates all security checks before allowing PR merge:

```yaml
ci-status:
  needs: [lint-and-format, test, build, security, ...]
  steps:
    - name: Check job results
      run: |
        if [[ "${{ needs.security.result }}" != "success" ]]; then
          echo "Security job failed - blocking merge"
          exit 1
        fi
```

### Dependency Vulnerability Response

When the CI pipeline detects vulnerabilities, follow these procedures:

#### Step 1: Assess Vulnerability

```bash
# View detailed vulnerability report
npm audit --audit-level=high

# Example output:
# lodash  <4.17.21
# Severity: high
# Prototype Pollution
# Fix available via `npm audit fix`
```

#### Step 2: Attempt Automatic Fix

```bash
# Try automatic fix (safe version bumps)
npm audit fix --audit-level=high

# For breaking changes, use force (review changes carefully!)
npm audit fix --force --audit-level=high

# Verify changes
git diff package.json package-lock.json
npm test
```

#### Step 3: Manual Remediation

If automatic fix is not available:

```bash
# 1. Update specific package
npm update lodash@latest

# 2. If transitive dependency, update parent
npm update --depth 2

# 3. Override with package.json resolutions (npm 8.3+)
{
  "overrides": {
    "lodash": "^4.17.21"
  }
}

# 4. Run tests to verify compatibility
npm test
```

#### Step 4: Document Accepted Risks

For vulnerabilities without available fixes:

1. **Create GitHub issue** documenting:
   - CVE identifier and severity
   - Affected package and version
   - Why fix is not available (no patch, breaking change)
   - Mitigation measures (if any)
   - Target remediation date

2. **Add to security-exceptions.md** (create if doesn't exist):

```markdown
## Accepted Security Exceptions

### CVE-2023-12345 (lodash@4.17.20)

- **Severity**: High
- **Status**: Accepted (temporary)
- **Reason**: No patch available, breaking change in v5
- **Mitigation**: Input validation prevents exploitation
- **Review Date**: 2026-03-01
- **Issue**: #1234
```

3. **Optional: Use npm audit exceptions** (npm 10+):

```bash
# Suppress specific vulnerability (use sparingly!)
npm audit fix --audit-level=high --ignore-vulnerability CVE-2023-12345
```

### Production Deployment Validation

Before deploying to production, run the preflight validation script:

```bash
./scripts/production-preflight.sh
```

**Security Validations Performed:**

1. **KEY_BACKEND Validation**

```bash
# BLOCKS deployment if KEY_BACKEND=env (insecure)
if [[ "${KEY_BACKEND}" == "env" ]]; then
  echo "❌ SECURITY ERROR: KEY_BACKEND=env not allowed in production"
  exit 1
fi

# Valid production options:
# - aws-kms
# - gcp-kms
# - azure-keyvault
```

2. **GRAFANA_PASSWORD Validation**

```bash
# BLOCKS deployment if default password detected
if [[ "${GRAFANA_PASSWORD}" == "admin" ]]; then
  echo "❌ SECURITY ERROR: Default Grafana password detected"
  exit 1
fi
```

3. **Dependency Audit**

```bash
# Checks for critical/high vulnerabilities
npm audit --audit-level=high --production

# Exit code 0 = No vulnerabilities
# Exit code 1 = Vulnerabilities found (blocks deployment)
```

**Preflight Script Usage:**

```bash
# Test configuration before deployment
KEY_BACKEND=aws-kms \
GRAFANA_PASSWORD=secure-password-here \
./scripts/production-preflight.sh

# Expected output on success:
# ✅ All production configuration checks passed
# Exit code: 0

# Expected output on failure:
# ❌ Production configuration validation FAILED
# Exit code: 1
```

### Security Scanning Tools

#### npm audit

**Built-in npm security scanner:**

```bash
# Scan all dependencies
npm audit

# Scan production dependencies only
npm audit --production

# Show JSON output for automation
npm audit --json

# Fix vulnerabilities automatically
npm audit fix

# Audit levels:
# - low: Minor issues, may not need immediate fix
# - moderate: Should fix in next release
# - high: Fix ASAP (blocks CI)
# - critical: Fix immediately (blocks CI)
```

**npm audit Exit Codes:**

| Exit Code | Meaning               | CI Action |
| --------- | --------------------- | --------- |
| 0         | No vulnerabilities    | Pass ✓    |
| 1         | Vulnerabilities found | Fail ✗    |

#### Snyk

**Advanced security platform (optional):**

```bash
# Setup (requires Snyk account)
npm install -g snyk
snyk auth

# Test for vulnerabilities
snyk test

# Monitor project (continuous monitoring)
snyk monitor

# Test with severity threshold
snyk test --severity-threshold=high

# Ignore specific vulnerability
snyk ignore --id=SNYK-JS-LODASH-12345
```

**Snyk Features:**

- Dependency vulnerability scanning
- License compliance checks
- Container vulnerability scanning
- Infrastructure as Code scanning
- Fix PRs (automatic vulnerability fixes)

**GitHub Integration:**

1. Add `SNYK_TOKEN` to GitHub Secrets
2. Snyk automatically creates PRs for vulnerabilities
3. Security tab shows vulnerability dashboard

### Security Update Procedures

#### Monthly Dependency Updates

```bash
# Check for outdated packages
npm outdated

# Update minor/patch versions
npm update

# Update major versions (review breaking changes!)
npm install package@latest

# Run full test suite
npm test

# Create PR with dependency updates
git checkout -b chore/monthly-dependency-updates
git add package.json package-lock.json
git commit -m "chore: monthly dependency updates"
git push origin chore/monthly-dependency-updates
```

#### Critical Security Patch Response (48-Hour SLA)

When a critical vulnerability is announced:

1. **Hour 0-2: Assessment**

   ```bash
   # Check if project is affected
   npm audit
   snyk test

   # Review CVE details
   # - Exploitability
   # - Attack vector
   # - Impact on M2M
   ```

2. **Hour 2-8: Fix Development**

   ```bash
   # Create hotfix branch
   git checkout -b hotfix/cve-2023-12345

   # Apply fix
   npm audit fix

   # Run tests
   npm test
   ```

3. **Hour 8-24: Testing & Review**

   ```bash
   # Integration tests
   npm run test:integration

   # Manual QA in staging
   # Security review
   # Peer code review
   ```

4. **Hour 24-48: Deployment**

   ```bash
   # Merge to main
   git merge hotfix/cve-2023-12345

   # Deploy to production
   ./scripts/production-deploy.sh

   # Monitor for issues
   # Post-deployment verification
   ```

### Dependabot Configuration

Enable automated dependency updates:

```yaml
# .github/dependabot.yml
version: 2
updates:
  - package-ecosystem: 'npm'
    directory: '/'
    schedule:
      interval: 'weekly'
    open-pull-requests-limit: 10
    reviewers:
      - 'security-team'
    labels:
      - 'dependencies'
      - 'security'
    # Auto-merge patch updates
    allow:
      - dependency-type: 'all'
    # Group minor updates
    groups:
      minor-updates:
        patterns:
          - '*'
        update-types:
          - 'minor'
          - 'patch'
```

**Dependabot Auto-Merge Policy:**

| Update Type | Auto-Merge | Review Required |
| ----------- | ---------- | --------------- |
| Patch       | Yes        | No              |
| Minor       | No         | Yes             |
| Major       | No         | Yes (thorough)  |
| Security    | Fast-track | Expedited       |

### Security Code Review Checklist

When reviewing PRs, verify:

- [ ] No hardcoded secrets (API keys, passwords, private keys)
- [ ] No sensitive data in logs (`logger.info(password)` ❌)
- [ ] Input validation on all external data
- [ ] SQL/NoSQL injection prevention (parameterized queries)
- [ ] XSS prevention (output encoding)
- [ ] CSRF protection on state-changing operations
- [ ] Rate limiting on public endpoints
- [ ] Authentication/authorization checks
- [ ] Secure random generation (`crypto.randomBytes`, not `Math.random()`)
- [ ] TLS/SSL certificate validation (no `rejectUnauthorized: false`)

### Secret Scanning

**Pre-Commit Hook (git-secrets):**

```bash
# Install git-secrets
brew install git-secrets  # macOS
apt install git-secrets   # Ubuntu

# Configure for repository
cd /path/to/m2m
git secrets --install
git secrets --register-aws
git secrets --add 'PRIVATE_KEY='
git secrets --add 'SECRET_KEY='
git secrets --add '[0-9a-f]{64}'  # 256-bit hex keys

# Scan repository history
git secrets --scan-history
```

**GitHub Secret Scanning:**

GitHub automatically scans commits for known secret patterns:

- API keys (AWS, GCP, Azure, GitHub)
- Private keys (RSA, ECDSA, Ed25519)
- Database connection strings
- OAuth tokens

If secrets are detected:

1. Secret is automatically revoked (if supported)
2. Repository admin is notified
3. Commit should be removed from history

**Remove Secrets from Git History:**

```bash
# Use BFG Repo-Cleaner (safer than git filter-branch)
brew install bfg

# Remove secrets
bfg --replace-text secrets.txt repo.git
cd repo.git
git reflog expire --expire=now --all
git gc --prune=now --aggressive

# Force push (coordinate with team!)
git push --force
```

---

## Network Security

### Firewall Configuration

#### Required Ports

| Port | Protocol | Direction | Purpose        | Restriction              |
| ---- | -------- | --------- | -------------- | ------------------------ |
| 4000 | TCP      | Inbound   | BTP WebSocket  | Peer IPs only            |
| 8080 | TCP      | Inbound   | Health/Metrics | Internal/Monitoring only |
| 443  | TCP      | Outbound  | Blockchain RPC | Allow                    |

#### UFW (Ubuntu Firewall)

```bash
# Reset and configure firewall
sudo ufw reset
sudo ufw default deny incoming
sudo ufw default allow outgoing

# Allow SSH (restrict to bastion/VPN IPs in production)
sudo ufw allow from 10.0.0.0/8 to any port 22 proto tcp

# Allow BTP from specific peer IPs
sudo ufw allow from 203.0.113.10 to any port 4000 proto tcp  # Peer 1
sudo ufw allow from 203.0.113.20 to any port 4000 proto tcp  # Peer 2
# Or allow from peer subnet
sudo ufw allow from 203.0.113.0/24 to any port 4000 proto tcp

# Allow health/metrics from internal monitoring only
sudo ufw allow from 10.0.0.0/8 to any port 8080 proto tcp

# Enable firewall
sudo ufw enable
sudo ufw status verbose
```

#### iptables (Advanced)

```bash
#!/bin/bash
# firewall-setup.sh

# Flush existing rules
iptables -F
iptables -X

# Default policies
iptables -P INPUT DROP
iptables -P FORWARD DROP
iptables -P OUTPUT ACCEPT

# Allow loopback
iptables -A INPUT -i lo -j ACCEPT

# Allow established connections
iptables -A INPUT -m state --state ESTABLISHED,RELATED -j ACCEPT

# Allow SSH from bastion only
iptables -A INPUT -p tcp --dport 22 -s 10.0.1.100/32 -j ACCEPT

# Allow BTP from peer IPs
iptables -A INPUT -p tcp --dport 4000 -s 203.0.113.10/32 -j ACCEPT
iptables -A INPUT -p tcp --dport 4000 -s 203.0.113.20/32 -j ACCEPT

# Allow health checks from monitoring subnet
iptables -A INPUT -p tcp --dport 8080 -s 10.0.10.0/24 -j ACCEPT

# Log dropped packets (for debugging)
iptables -A INPUT -j LOG --log-prefix "DROPPED: " --log-level 4

# Save rules
iptables-save > /etc/iptables/rules.v4
```

### Network Segmentation

```
┌─────────────────────────────────────────────────────────────┐
│                    Production VPC                            │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  ┌──────────────────┐    ┌──────────────────┐              │
│  │  Public Subnet   │    │  Private Subnet  │              │
│  │  10.0.1.0/24     │    │  10.0.2.0/24     │              │
│  ├──────────────────┤    ├──────────────────┤              │
│  │ • Load Balancer  │───▶│ • Connector      │              │
│  │ • Bastion Host   │    │ • TigerBeetle    │              │
│  └──────────────────┘    └────────┬─────────┘              │
│                                   │                         │
│                          ┌────────▼─────────┐              │
│                          │ Database Subnet  │              │
│                          │  10.0.3.0/24     │              │
│                          ├──────────────────┤              │
│                          │ • TigerBeetle    │              │
│                          │   (if separate)  │              │
│                          └──────────────────┘              │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

---

## TLS/HTTPS Configuration

### Certificate Generation

#### Let's Encrypt (Production)

```bash
# Install certbot
sudo apt install certbot

# Generate certificate
sudo certbot certonly --standalone \
  -d connector.example.com \
  --email admin@example.com \
  --agree-tos \
  --non-interactive

# Certificates stored at:
# /etc/letsencrypt/live/connector.example.com/fullchain.pem
# /etc/letsencrypt/live/connector.example.com/privkey.pem

# Set up auto-renewal
sudo systemctl enable certbot.timer
sudo systemctl start certbot.timer

# Test renewal
sudo certbot renew --dry-run
```

#### Self-Signed (Testing Only)

```bash
# Generate self-signed certificate (NOT for production)
openssl req -x509 -nodes -days 365 -newkey rsa:4096 \
  -keyout /etc/ssl/private/connector.key \
  -out /etc/ssl/certs/connector.crt \
  -subj "/CN=connector.example.com/O=M2M/C=US"

# Set permissions
chmod 600 /etc/ssl/private/connector.key
chmod 644 /etc/ssl/certs/connector.crt
```

### TLS Termination Options

#### Option 1: Nginx Reverse Proxy (Recommended)

```nginx
# /etc/nginx/sites-available/agent-runtime

upstream connector_btp {
    server 127.0.0.1:4000;
}

upstream connector_health {
    server 127.0.0.1:8080;
}

# Redirect HTTP to HTTPS
server {
    listen 80;
    server_name connector.example.com;
    return 301 https://$server_name$request_uri;
}

# HTTPS server
server {
    listen 443 ssl http2;
    server_name connector.example.com;

    # TLS certificates
    ssl_certificate /etc/letsencrypt/live/connector.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/connector.example.com/privkey.pem;

    # TLS configuration (Mozilla Modern)
    ssl_protocols TLSv1.3;
    ssl_prefer_server_ciphers off;

    # HSTS
    add_header Strict-Transport-Security "max-age=63072000" always;

    # WebSocket proxy for BTP
    location /btp {
        proxy_pass http://connector_btp;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_read_timeout 86400;
    }

    # Health check (internal only)
    location /health {
        allow 10.0.0.0/8;
        deny all;
        proxy_pass http://connector_health/health;
    }

    # Metrics (internal only)
    location /metrics {
        allow 10.0.0.0/8;
        deny all;
        proxy_pass http://connector_health/metrics;
    }
}
```

#### Option 2: Traefik (Docker-Native)

```yaml
# docker-compose-production.yml with Traefik

services:
  traefik:
    image: traefik:v2.10
    command:
      - '--api.insecure=false'
      - '--providers.docker=true'
      - '--entrypoints.websecure.address=:443'
      - '--certificatesresolvers.letsencrypt.acme.tlschallenge=true'
      - '--certificatesresolvers.letsencrypt.acme.email=admin@example.com'
      - '--certificatesresolvers.letsencrypt.acme.storage=/letsencrypt/acme.json'
    ports:
      - '443:443'
    volumes:
      - '/var/run/docker.sock:/var/run/docker.sock:ro'
      - 'letsencrypt:/letsencrypt'

  connector:
    image: agent-runtime/connector:latest
    labels:
      - 'traefik.enable=true'
      - 'traefik.http.routers.connector.rule=Host(`connector.example.com`)'
      - 'traefik.http.routers.connector.entrypoints=websecure'
      - 'traefik.http.routers.connector.tls.certresolver=letsencrypt'
      - 'traefik.http.services.connector.loadbalancer.server.port=4000'

volumes:
  letsencrypt:
```

### TLS Best Practices

| Setting              | Recommendation                 |
| -------------------- | ------------------------------ |
| Minimum TLS Version  | TLS 1.2 (prefer TLS 1.3)       |
| Cipher Suites        | ECDHE with AES-GCM or ChaCha20 |
| Key Size             | RSA 2048+ or ECDSA P-256+      |
| Certificate Validity | 90 days (Let's Encrypt)        |
| HSTS                 | Enabled with min 1 year        |
| OCSP Stapling        | Enabled                        |

### Certificate Renewal Automation

```bash
#!/bin/bash
# /etc/cron.d/certbot-renewal

# Renew certificates twice daily
0 0,12 * * * root certbot renew --quiet --post-hook "systemctl reload nginx"
```

---

## Key Management (HSM/KMS)

### Cloud KMS Configuration

#### AWS KMS

```bash
# Environment configuration
KEY_BACKEND=aws-kms
AWS_REGION=us-east-1
AWS_KMS_EVM_KEY_ID=arn:aws:kms:us-east-1:123456789012:key/12345678-1234-1234-1234-123456789012
```

**IAM Policy for Connector:**

```json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Action": ["kms:Sign", "kms:GetPublicKey", "kms:DescribeKey"],
      "Resource": [
        "arn:aws:kms:us-east-1:123456789012:key/12345678-*",
        "arn:aws:kms:us-east-1:123456789012:key/87654321-*"
      ]
    }
  ]
}
```

**Key Rotation:**

```bash
# Enable automatic key rotation (annually)
aws kms enable-key-rotation --key-id $AWS_KMS_EVM_KEY_ID

# Verify rotation status
aws kms get-key-rotation-status --key-id $AWS_KMS_EVM_KEY_ID
```

#### GCP Cloud KMS

```bash
# Environment configuration
KEY_BACKEND=gcp-kms
GCP_PROJECT_ID=my-project
GCP_LOCATION_ID=us-east1
GCP_KEY_RING_ID=connector-keyring
GCP_KMS_EVM_KEY_ID=evm-signing-key
```

**Setup Commands:**

```bash
# Create key ring
gcloud kms keyrings create connector-keyring \
  --location=us-east1 \
  --project=my-project

# Create EVM signing key (secp256k1)
gcloud kms keys create evm-signing-key \
  --keyring=connector-keyring \
  --location=us-east1 \
  --purpose=asymmetric-signing \
  --default-algorithm=ec-sign-secp256k1-sha256

# Set up rotation (30 days)
gcloud kms keys update evm-signing-key \
  --keyring=connector-keyring \
  --location=us-east1 \
  --rotation-period=2592000s \
  --next-rotation-time=2026-02-01T00:00:00Z
```

#### Azure Key Vault

```bash
# Environment configuration
KEY_BACKEND=azure-kv
AZURE_VAULT_URL=https://agent-runtime-vault.vault.azure.net
AZURE_EVM_KEY_NAME=evm-signing-key
AZURE_TENANT_ID=00000000-0000-0000-0000-000000000000
AZURE_CLIENT_ID=00000000-0000-0000-0000-000000000000
```

**Setup Commands:**

```bash
# Create Key Vault
az keyvault create \
  --name agent-runtime-vault \
  --resource-group m2m-production \
  --location eastus \
  --sku premium  # Use premium for HSM-backed keys

# Create EVM signing key
az keyvault key create \
  --vault-name agent-runtime-vault \
  --name evm-signing-key \
  --kty EC \
  --curve P-256K

# Set up rotation policy
az keyvault key rotation-policy update \
  --vault-name agent-runtime-vault \
  --name evm-signing-key \
  --value @rotation-policy.json
```

---

## Secrets Management

### Environment Variable Security

**Never do this:**

```bash
# BAD: Secrets in .env file committed to git
PRIVATE_KEY=0x1234567890abcdef...
```

**Best Practices:**

1. **Use Secrets Manager**

```bash
# Store secret in AWS Secrets Manager
aws secretsmanager create-secret \
  --name agent-runtime/connector/api-key \
  --secret-string "your-api-key"

# Reference in application (not .env)
API_KEY=$(aws secretsmanager get-secret-value --secret-id agent-runtime/connector/api-key --query SecretString --output text)
```

2. **Docker Secrets (Swarm)**

```yaml
# docker-compose.yml
services:
  connector:
    secrets:
      - kms_credentials
    environment:
      - KMS_CREDENTIALS_FILE=/run/secrets/kms_credentials

secrets:
  kms_credentials:
    external: true
```

3. **Kubernetes Secrets**

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: connector-secrets
type: Opaque
data:
  api-key: <base64-encoded-value>
---
apiVersion: apps/v1
kind: Deployment
spec:
  template:
    spec:
      containers:
        - name: connector
          envFrom:
            - secretRef:
                name: connector-secrets
```

### Secret Rotation

```bash
#!/bin/bash
# rotate-secrets.sh

# Rotate API keys
NEW_KEY=$(openssl rand -hex 32)

# Update in secrets manager
aws secretsmanager update-secret \
  --secret-id agent-runtime/connector/api-key \
  --secret-string "$NEW_KEY"

# Restart connector to pick up new secret
docker compose -f docker-compose-production.yml restart connector

# Verify connector is healthy
sleep 10
curl -sf http://localhost:8080/health || exit 1
```

---

## Authentication and Authorization

### BTP Peer Authentication

```yaml
# examples/production-single-node.yaml
peers:
  - id: trusted-peer
    relation: peer
    btpUrl: wss://peer.example.com:4000
    auth:
      type: shared-secret
      secretEnvVar: BTP_PEER_TRUSTED_SECRET # Reference env var, don't inline
```

### API Authentication

```bash
# Health/metrics endpoints should be restricted

# Option 1: Network-level restriction (recommended)
# Configure firewall to only allow monitoring IPs

# Option 2: Basic auth (if network restriction not possible)
# Configure in nginx:
location /metrics {
    auth_basic "Metrics";
    auth_basic_user_file /etc/nginx/.htpasswd;
    proxy_pass http://connector:8080/metrics;
}
```

### RBAC Model

| Role     | Permissions                      | Use Case              |
| -------- | -------------------------------- | --------------------- |
| Viewer   | Read health, metrics             | Monitoring dashboards |
| Operator | Viewer + restart, config reload  | On-call engineers     |
| Admin    | Operator + configuration changes | Platform team         |
| Auditor  | Read all logs, metrics           | Compliance            |

---

## Audit Logging

### Configuration

```bash
# Enable comprehensive audit logging
AUDIT_LOG_ENABLED=true
AUDIT_LOG_LEVEL=info
AUDIT_LOG_PATH=/var/log/m2m/audit.log
AUDIT_LOG_RETENTION_DAYS=365
```

### Audit Events

| Event                | Description            | Fields                  |
| -------------------- | ---------------------- | ----------------------- |
| `auth.login`         | Authentication attempt | user, success, ip       |
| `config.change`      | Configuration modified | field, old, new, user   |
| `settlement.execute` | Settlement triggered   | amount, peer, method    |
| `peer.connect`       | Peer connection        | peerId, success, reason |
| `key.access`         | KMS key accessed       | keyId, operation        |
| `wallet.create`      | Wallet created         | agentId, timestamp      |

### Log Format

```json
{
  "timestamp": "2026-01-23T10:30:00.000Z",
  "level": "info",
  "event": "settlement.execute",
  "audit": true,
  "nodeId": "connector-1",
  "data": {
    "peerId": "peer-123",
    "amount": "1000000",
    "method": "evm",
    "success": true
  },
  "metadata": {
    "correlationId": "txn_abc123",
    "ip": "10.0.1.50"
  }
}
```

### Log Shipping

```yaml
# filebeat.yml for log aggregation
filebeat.inputs:
  - type: log
    enabled: true
    paths:
      - /var/log/m2m/audit.log
    json.keys_under_root: true
    json.add_error_key: true
    fields:
      log_type: audit
    fields_under_root: true

output.elasticsearch:
  hosts: ['elasticsearch:9200']
  index: 'm2m-audit-%{+yyyy.MM.dd}'

# Retention policy: 7 years for regulatory compliance
```

---

## Security Monitoring and Alerting

### Security-Specific Alerts

```yaml
# monitoring/prometheus/alerts/security-alerts.yml
groups:
  - name: security
    rules:
      - alert: HighAuthFailureRate
        expr: rate(auth_failures_total[5m]) > 10
        for: 2m
        labels:
          severity: critical
        annotations:
          summary: 'High authentication failure rate'
          description: '{{ $value }} auth failures per second'

      - alert: UnauthorizedAccessAttempt
        expr: increase(unauthorized_access_total[1m]) > 0
        labels:
          severity: critical
        annotations:
          summary: 'Unauthorized access attempt detected'

      - alert: KMSAccessAnomaly
        expr: rate(kms_operations_total[5m]) > 100
        for: 1m
        labels:
          severity: warning
        annotations:
          summary: 'Unusual KMS access pattern'

      - alert: AuditLogGap
        expr: time() - audit_log_last_write_timestamp > 300
        labels:
          severity: critical
        annotations:
          summary: 'Audit logging may be failing'

      - alert: TLSCertExpiringSoon
        expr: (probe_ssl_earliest_cert_expiry - time()) / 86400 < 14
        labels:
          severity: warning
        annotations:
          summary: 'TLS certificate expires in {{ $value }} days'
```

### Security Dashboard Metrics

```promql
# Failed authentication rate
rate(auth_failures_total[5m])

# Successful vs failed auth ratio
sum(auth_successes_total) / (sum(auth_successes_total) + sum(auth_failures_total))

# KMS operations per minute
rate(kms_operations_total[1m]) * 60

# Unauthorized access attempts
increase(unauthorized_access_total[1h])

# Certificate days until expiry
(probe_ssl_earliest_cert_expiry - time()) / 86400
```

---

## Container Security

### Docker Security Best Practices

```dockerfile
# Dockerfile security hardening

# Use specific version, not 'latest'
FROM node:20.11.0-alpine

# Run as non-root user
RUN addgroup -S connector && adduser -S connector -G connector
USER connector

# Don't store secrets in image
# Use runtime secrets instead

# Minimize attack surface
RUN apk --no-cache add --virtual .build-deps \
    && npm ci --only=production \
    && apk del .build-deps

# Health check
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD curl -f http://localhost:8080/health/live || exit 1
```

### Docker Compose Security

```yaml
# docker-compose-production.yml
services:
  connector:
    image: agent-runtime/connector:v1.2.0
    user: '1000:1000' # Run as non-root
    read_only: true # Read-only filesystem
    security_opt:
      - no-new-privileges:true
    cap_drop:
      - ALL
    cap_add:
      - NET_BIND_SERVICE
    tmpfs:
      - /tmp:noexec,nosuid,nodev
    volumes:
      - type: volume
        source: connector-data
        target: /data
        read_only: false
```

### Container Scanning

```bash
# Scan image for vulnerabilities
docker scout cves agent-runtime/connector:v1.2.0

# Or use Trivy
trivy image agent-runtime/connector:v1.2.0

# Fail CI on critical vulnerabilities
trivy image --exit-code 1 --severity CRITICAL agent-runtime/connector:v1.2.0
```

---

## Production Security Audit Checklist

Complete this checklist before production deployment:

### CI/CD Security ✓ (Story 16.2)

- [ ] **npm audit blocking** - CI fails on high/critical vulnerabilities
- [ ] **Snyk scan blocking** - CI fails on high/critical findings
- [ ] **ci-status validates security** - Security job checked in CI gate
- [ ] **No critical CVEs** - All dependencies scanned and clean
- [ ] **Dependabot enabled** - Automated dependency updates configured
- [ ] **Production preflight validated** - KEY_BACKEND and GRAFANA_PASSWORD checked
- [ ] **Security exceptions documented** - Any accepted risks in writing
- [ ] **Patch response plan** - Critical vulnerability SLA defined (48h)

### Network Security ✓

- [ ] **Firewall configured** - Only required ports open
- [ ] **Default deny policy** - All other traffic blocked
- [ ] **Peer IPs whitelisted** - BTP port restricted to known peers
- [ ] **Health endpoint restricted** - Internal access only
- [ ] **SSH access restricted** - Bastion/VPN only
- [ ] **Network segmentation** - Connector in private subnet

### TLS/Encryption ✓

- [ ] **TLS 1.2+ enforced** - No SSLv3, TLS 1.0, TLS 1.1
- [ ] **Valid certificates** - Not self-signed in production
- [ ] **Auto-renewal configured** - Let's Encrypt or similar
- [ ] **HSTS enabled** - Strict-Transport-Security header
- [ ] **Certificate monitoring** - Alerts before expiry

### Key Management ✓

- [ ] **Cloud KMS configured** - No `KEY_BACKEND=env` in production
- [ ] **Key rotation enabled** - Automatic rotation configured
- [ ] **IAM least privilege** - Minimal permissions for connector
- [ ] **Key access audited** - All KMS operations logged
- [ ] **Backup keys secured** - Separate from primary keys

### Secrets Management ✓

- [ ] **No secrets in code** - All secrets in secrets manager
- [ ] **No secrets in logs** - Sanitization enabled
- [ ] **.env not in git** - Added to .gitignore
- [ ] **Secret rotation** - Process documented and tested
- [ ] **Access restricted** - Only authorized personnel

### Authentication ✓

- [ ] **Peer authentication enabled** - Shared secrets configured
- [ ] **API endpoints secured** - Network or auth restriction
- [ ] **Admin access audited** - All admin actions logged
- [ ] **MFA enabled** - For all human access (SSH, console)

### Audit and Monitoring ✓

- [ ] **Audit logging enabled** - All security events logged
- [ ] **Log retention configured** - Minimum 1 year
- [ ] **SIEM integration** - Logs shipped to central system
- [ ] **Security alerts configured** - Auth failures, unauthorized access
- [ ] **Incident response plan** - Documented and tested

### Container Security ✓

- [ ] **Non-root user** - Container runs as unprivileged user
- [ ] **Read-only filesystem** - Where possible
- [ ] **Vulnerability scanning** - No critical CVEs
- [ ] **Image signing** - Trusted images only
- [ ] **Resource limits** - CPU/memory limits set

### Compliance ✓

- [ ] **Data residency** - Compliant with regulations
- [ ] **Encryption at rest** - Database encrypted
- [ ] **Access controls** - RBAC implemented
- [ ] **Audit trail** - Complete and tamper-evident
- [ ] **Security review** - External audit completed

---

## Security Incident Response

### Immediate Actions

1. **Isolate** - Disconnect affected systems
2. **Preserve** - Capture logs and evidence
3. **Notify** - Alert security team and management
4. **Contain** - Prevent further damage
5. **Investigate** - Determine scope and root cause

### Contact Information

| Role             | Contact                         | Escalation      |
| ---------------- | ------------------------------- | --------------- |
| On-Call Security | security-oncall@example.com     | PagerDuty       |
| Security Team    | security@example.com            | Slack #security |
| Management       | security-escalation@example.com | Phone           |

See [incident-response-runbook.md](./incident-response-runbook.md) for detailed procedures.

---

## Support Resources

- **Production Deployment**: [production-deployment-guide.md](./production-deployment-guide.md)
- **Monitoring Setup**: [monitoring-setup-guide.md](./monitoring-setup-guide.md)
- **Incident Response**: [incident-response-runbook.md](./incident-response-runbook.md)
- **Backup/Recovery**: [backup-disaster-recovery.md](./backup-disaster-recovery.md)

---

**Document Version**: 1.1
**Last Updated**: 2026-02-02
**Authors**: Dev Agent James (Story 12.9, Story 16.2)
**Changelog**:

- v1.1 (2026-02-02): Added CI/CD Security Pipeline section (Story 16.2)
- v1.0 (2026-01-23): Initial version (Story 12.9)
