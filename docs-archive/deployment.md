# Deployment Guide

This guide covers deploying Agent Runtime with Docker Compose and Kubernetes, including secrets management and custom token configuration.

---

## Quick Reference

| Environment        | Command             | Secrets     | Token Config     |
| ------------------ | ------------------- | ----------- | ---------------- |
| **Local Dev**      | `npm run dev`       | `.env` file | Local Anvil      |
| **Docker Compose** | `docker-compose up` | `.env` file | Testnet/Mainnet  |
| **Kubernetes**     | `kubectl apply -k`  | K8s Secrets | ConfigMap/Secret |

---

## Docker Compose Deployment

### 1. Prepare Environment File

```bash
# Copy the example configuration
cp .env.example .env

# Edit with your values
nano .env
```

### 2. Configure Secrets

**Development (local testing):**

```env
# Key management - env backend for development only
KEY_BACKEND=env

# EVM private key (hex format)
EVM_PRIVATE_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
```

**Production (KMS required):**

```env
# AWS KMS
KEY_BACKEND=aws-kms
AWS_REGION=us-east-1
AWS_KMS_EVM_KEY_ID=arn:aws:kms:us-east-1:123456789012:key/xxxxx

# GCP KMS
KEY_BACKEND=gcp-kms
GCP_PROJECT_ID=my-project
GCP_LOCATION_ID=us-east1
GCP_KEY_RING_ID=connector-keyring
GCP_KMS_EVM_KEY_ID=evm-signing-key

# Azure Key Vault
KEY_BACKEND=azure-keyvault
AZURE_VAULT_URL=https://my-vault.vault.azure.net
AZURE_EVM_KEY_NAME=evm-signing-key
```

### 3. Configure Blockchain Networks

Use `NETWORK_MODE` to switch between testnet and mainnet:

```env
# Set network mode: 'testnet' (default) or 'mainnet'
NETWORK_MODE=testnet
```

**Network Mode URL Mappings:**

| Chain   | Testnet                    | Mainnet                    |
| ------- | -------------------------- | -------------------------- |
| Base L2 | `https://sepolia.base.org` | `https://mainnet.base.org` |

**Using the deploy script (recommended):**

```bash
# Testnet deployment (default)
./scripts/deploy-5-peer-multihop.sh

# Mainnet deployment
NETWORK_MODE=mainnet ./scripts/deploy-5-peer-multihop.sh
```

### 4. Configure Custom Token (Base/EVM)

To use your own ERC-20 token:

```env
# Your custom ERC-20 token contract address
M2M_TOKEN_ADDRESS=0xYourTokenContractAddress

# Token Network Registry (Raiden-style payment channels)
TOKEN_NETWORK_REGISTRY=0xYourRegistryContractAddress

# Settlement configuration
SETTLEMENT_ENABLED=true
SETTLEMENT_THRESHOLD=1000000  # In token base units
```

**Deploying Custom Contracts:**

```bash
cd packages/contracts

# Deploy token
npx hardhat run scripts/deploy-token.ts --network base-sepolia

# Deploy registry
npx hardhat run scripts/deploy-registry.ts --network base-sepolia
```

### 5. Start Services

```bash
# Single connector
docker-compose up -d

# 5-peer multi-hop network
docker-compose -f docker-compose-5-peer-multihop.yml up -d --build

# View logs
docker-compose logs -f
```

### 6. Verify Deployment

```bash
# Check health
curl http://localhost:9080/health

# Access Explorer UI
open http://localhost:5173
```

---

## Kubernetes Deployment

### 1. Prerequisites

- Kubernetes cluster (1.25+)
- kubectl configured
- kustomize (built into kubectl)

### 2. Deploy TigerBeetle

```bash
kubectl apply -k k8s/tigerbeetle/base
kubectl -n tigerbeetle get pods
```

### 3. Create Secrets

**Option A: Direct kubectl (development):**

```bash
kubectl -n agent-runtime create secret generic connector-secrets \
  --from-literal=EVM_PRIVATE_KEY=0xYourPrivateKey
```

**Option B: Sealed Secrets (production):**

```bash
kubectl apply -f https://github.com/bitnami-labs/sealed-secrets/releases/download/v0.24.0/controller.yaml
kubeseal --format=yaml < k8s/connector/base/secret.yaml > sealed-secret.yaml
kubectl apply -f sealed-secret.yaml
```

**Option C: External Secrets Operator (production):**

```yaml
apiVersion: external-secrets.io/v1beta1
kind: ExternalSecret
metadata:
  name: connector-secrets
  namespace: agent-runtime
spec:
  refreshInterval: 1h
  secretStoreRef:
    name: aws-secrets-manager
    kind: ClusterSecretStore
  target:
    name: connector-secrets
  data:
    - secretKey: EVM_PRIVATE_KEY
      remoteRef:
        key: agent-runtime/connector/evm-key
```

### 4. Choose Network (Testnet vs Mainnet)

| Overlay      | Network Mode | Networks     |
| ------------ | ------------ | ------------ |
| `staging`    | **Testnet**  | Base Sepolia |
| `production` | **Mainnet**  | Base Mainnet |

```bash
# Testnet
kubectl apply -k k8s/connector/overlays/staging

# Mainnet
kubectl apply -k k8s/connector/overlays/production
```

### 5. Deploy and Verify

```bash
kubectl -n agent-runtime get pods
kubectl -n agent-runtime logs -f deployment/connector
```

### 6. Expose Services

```bash
# Port-forward
kubectl -n agent-runtime port-forward svc/connector 4000:4000
kubectl -n agent-runtime port-forward svc/connector 5173:5173
```

---

## Environment Variables Reference

### Core Settings

| Variable                | Description                         | Default     |
| ----------------------- | ----------------------------------- | ----------- |
| `NODE_ID`               | Unique connector identifier         | `connector` |
| `LOG_LEVEL`             | Logging level                       | `info`      |
| `SETTLEMENT_PREFERENCE` | Settlement chain                    | `evm`       |
| `NETWORK_MODE`          | Network selection (testnet/mainnet) | `testnet`   |

### Key Management

| Variable          | Description            | Values                                        |
| ----------------- | ---------------------- | --------------------------------------------- |
| `KEY_BACKEND`     | Secret storage backend | `env`, `aws-kms`, `gcp-kms`, `azure-keyvault` |
| `EVM_PRIVATE_KEY` | EVM signing key        | `0x...`                                       |

### Settlement

| Variable               | Description                      | Default   |
| ---------------------- | -------------------------------- | --------- |
| `SETTLEMENT_ENABLED`   | Enable automatic settlement      | `true`    |
| `SETTLEMENT_THRESHOLD` | Balance threshold for settlement | `1000000` |

---

## Production Checklist

Before deploying to production:

- [ ] `KEY_BACKEND` is NOT `env` (use KMS)
- [ ] Using HTTPS RPC endpoints
- [ ] Secrets managed via KMS/Vault
- [ ] TigerBeetle has 3+ replicas
- [ ] Network policies configured
- [ ] Resource limits set
- [ ] Monitoring/alerting configured

Run preflight validation:

```bash
./scripts/production-preflight.sh
```
