# Local vs Production Configuration Guide

## Introduction

This guide explains how to configure the M2M connector for different deployment environments (development, staging, production) with a focus on blockchain configuration for Base L2 (EVM).

**Why Environment Separation Matters:**

- **Prevents accidental mainnet deployment** with development credentials
- **Protects against using test private keys in production** (funds at risk)
- **Ensures correct blockchain network selection** (local vs testnet vs mainnet)
- **Validates configuration before connector startup** (fail-safe design)

**Configuration Precedence:**

The connector loads configuration from multiple sources with the following precedence (highest to lowest priority):

1. **Environment variables** (shell exports, docker-compose environment section)
2. **.env files** (loaded by docker-compose --env-file)
3. **Code defaults** (environment-specific defaults in ConfigLoader)

## Configuration Precedence

Understanding configuration precedence is critical for troubleshooting and deployment.

### Precedence Order Table

| Priority    | Source                      | Example                           | Notes                                        |
| ----------- | --------------------------- | --------------------------------- | -------------------------------------------- |
| 1 (Highest) | Shell environment variables | `export BASE_RPC_URL=...`         | Set before running docker-compose            |
| 2           | .env file                   | `BASE_RPC_URL=http://anvil:8545`  | Loaded by docker-compose --env-file          |
| 3 (Lowest)  | Code defaults               | `development → http://anvil:8545` | Applied by ConfigLoader when env var missing |

### Example: Override Behavior

**Scenario:** You want to use a custom RPC endpoint instead of the local Anvil node.

**.env.dev file:**

```bash
BASE_RPC_URL=http://anvil:8545  # Default local Anvil
```

**Shell override:**

```bash
export BASE_RPC_URL=https://base-sepolia.g.alchemy.com/v2/YOUR_API_KEY
docker-compose -f docker-compose-dev.yml up
```

**Result:** Connector uses Alchemy endpoint (shell env var wins).

### How to Verify Active Configuration

Check connector startup logs for environment warnings that display active configuration:

```bash
docker-compose logs connector-a | grep "⚠️"
```

**Development mode logs:**

```
⚠️  DEVELOPMENT MODE - Using local blockchain nodes
⚠️  This is NOT production configuration
⚠️  Base RPC: http://anvil:8545
⚠️  Base Chain ID: 84532
```

**Production mode:** No warning logs (validation passes silently).

## Development Configuration

Development configuration uses **local blockchain nodes** running in Docker containers (Anvil for Base L2). This enables fast iteration without relying on public testnets or consuming mainnet resources.

### Default Blockchain Endpoints

| Blockchain | Service | Endpoint            | Network           | Notes     |
| ---------- | ------- | ------------------- | ----------------- | --------- |
| Base L2    | Anvil   | `http://anvil:8545` | Base Sepolia fork | Story 7.1 |

### Environment Variable Reference (Development)

| Variable                | Default             | Description               | Required          |
| ----------------------- | ------------------- | ------------------------- | ----------------- |
| `ENVIRONMENT`           | `development`       | Environment selector      | Yes               |
| `BASE_ENABLED`          | `true`              | Enable Base blockchain    | Yes               |
| `BASE_RPC_URL`          | `http://anvil:8545` | Base RPC endpoint         | No (uses default) |
| `BASE_CHAIN_ID`         | `84532`             | Base Sepolia chain ID     | No (uses default) |
| `BASE_PRIVATE_KEY`      | (Anvil Account #0)  | Private key for contracts | No (for Epic 8+)  |
| `BASE_REGISTRY_ADDRESS` | (empty)             | Payment channel registry  | No (for Epic 8)   |

### Quick Start: Development Setup

**Step 1:** Copy environment variable template

```bash
cp .env.dev.example .env.dev
```

**Step 2:** Start local blockchain nodes and connectors

```bash
make dev-up
```

**Step 3:** Verify blockchain services are running

```bash
# Check Anvil (Base L2)
docker-compose logs anvil | grep "Listening"
# Expected: Listening on http://0.0.0.0:8545

```

**Step 4:** Verify connector blockchain configuration

```bash
docker-compose logs connector-a | grep "⚠️"
# Expected: Development mode warnings with blockchain endpoints
```

**Detailed Setup:** See [Local Blockchain Development Guide](./local-blockchain-development.md)

## Production Configuration

Production configuration uses **public mainnet RPC endpoints** for Base L2. Production environment enforces strict validation rules to prevent misconfiguration.

### Default Blockchain Endpoints

| Blockchain | Service    | Endpoint                   | Network      | Notes                      |
| ---------- | ---------- | -------------------------- | ------------ | -------------------------- |
| Base L2    | Public RPC | `https://mainnet.base.org` | Base mainnet | Free, may have rate limits |

**Recommended Production RPC Providers:**

- **Alchemy:** `https://base-mainnet.g.alchemy.com/v2/YOUR_API_KEY`
- **Infura:** `https://base-mainnet.infura.io/v3/YOUR_PROJECT_ID`
- **QuickNode:** Custom dedicated endpoints with SLA guarantees

### Environment Variable Reference (Production)

| Variable                | Default                    | Description               | Required     | Security Notes                     |
| ----------------------- | -------------------------- | ------------------------- | ------------ | ---------------------------------- |
| `ENVIRONMENT`           | `production`               | Environment selector      | **Yes**      | Must be 'production'               |
| `BASE_ENABLED`          | `true`                     | Enable Base blockchain    | Yes          |                                    |
| `BASE_RPC_URL`          | `https://mainnet.base.org` | Base RPC endpoint         | No (default) | Use dedicated RPC (Alchemy/Infura) |
| `BASE_CHAIN_ID`         | `8453`                     | Base mainnet chain ID     | No (default) | **Must be 8453**                   |
| `BASE_PRIVATE_KEY`      | (empty)                    | Private key for contracts | **Yes**      | **From KMS/HSM only**              |
| `BASE_REGISTRY_ADDRESS` | (empty)                    | Payment channel registry  | **Yes**      | Deployed contract address          |

### Security Best Practices

#### 1. **Use AWS Secrets Manager or HashiCorp Vault for Private Keys**

**Example: AWS Secrets Manager**

```bash
# Store private key in AWS Secrets Manager
aws secretsmanager create-secret \
  --name m2m/base/private-key \
  --secret-string "0x..."

# Retrieve at runtime (in container entrypoint)
export BASE_PRIVATE_KEY=$(aws secretsmanager get-secret-value \
  --secret-id m2m/base/private-key \
  --query SecretString \
  --output text)
```

#### 2. **NEVER Commit Production .env Files to Git**

**.gitignore should include:**

```
.env.production  # Production environment variables (NEVER commit)
```

**Verification:**

```bash
git status --ignored | grep .env.production
# Should show: .env.production (ignored)
```

#### 3. **Use Dedicated RPC Endpoints with API Keys**

**Free public RPC endpoints have limitations:**

- **Rate limits:** 100-300 requests/second
- **No SLA:** Downtime possible
- **No support:** Community-only

**Dedicated RPC providers offer:**

- **Higher rate limits:** 1000+ requests/second
- **99.9% uptime SLA**
- **Priority support**

**Example: Alchemy Configuration**

```bash
BASE_RPC_URL=https://base-mainnet.g.alchemy.com/v2/YOUR_API_KEY
```

#### 4. **Rotate Keys Quarterly or After Compromise**

**Key rotation checklist:**

1. Generate new private key using hardware wallet or secure KMS
2. Update key in AWS Secrets Manager / Vault
3. Update `BASE_REGISTRY_ADDRESS` if deploying new contract
4. Restart connector containers to load new key
5. Archive old key securely (do not delete immediately)
6. Monitor for unexpected transactions from old key

#### 5. **Enable CloudWatch/Datadog Monitoring for RPC Health**

**Monitoring alerts to configure:**

- RPC endpoint latency > 500ms (5-minute average)
- RPC error rate > 1% (requests returning errors)
- Rate limit warnings (429 HTTP responses)
- Chain ID mismatch detected (logs contain "Chain ID mismatch")

## Staging Environment

Staging environment uses **public testnets** (Base Sepolia) for final validation before mainnet deployment.

### Default Blockchain Endpoints

| Blockchain | Service        | Endpoint                   | Network              |
| ---------- | -------------- | -------------------------- | -------------------- |
| Base L2    | Public testnet | `https://sepolia.base.org` | Base Sepolia testnet |

### Environment Variables (Staging)

```bash
# Environment selection
ENVIRONMENT=staging

# Base L2 Staging Configuration
BASE_ENABLED=true
BASE_RPC_URL=https://sepolia.base.org
BASE_CHAIN_ID=84532  # Base Sepolia
BASE_PRIVATE_KEY=  # Test private key (NOT production key)
BASE_REGISTRY_ADDRESS=  # Deployed on Base Sepolia

```

### What to Validate in Staging

**Use staging to validate:**

1. **Contract deployment on public testnet**
   - Deploy payment channel registry to Base Sepolia
   - Verify contract address and ABI match production expectations
   - Test contract interactions (create channel, settle, close)

2. **Payment channel creation/settlement workflows**
   - Test channel funding, claims, and settlement
   - Verify payment channel state transitions

3. **Integration with real testnet faucets**
   - Use Base Sepolia faucet for ETH
   - Verify transactions with real testnet validators (not local genesis accounts)

4. **RPC endpoint reliability**
   - Test under load (multiple concurrent requests)
   - Measure latency and error rates
   - Validate failover behavior if using multiple endpoints

## Environment Switching

### How to Switch from Development to Production

**Step 1:** Copy production environment template

```bash
cp .env.production.example .env.production
```

**Step 2:** Fill in required production values

**Edit .env.production:**

```bash
# CRITICAL: Use secure keys from KMS/HSM
BASE_PRIVATE_KEY=  # Generate with: openssl rand -hex 32
BASE_REGISTRY_ADDRESS=  # Deployed contract address
```

**Step 3:** Set ENVIRONMENT=production

```bash
# Verify in .env.production
grep ENVIRONMENT .env.production
# Should output: ENVIRONMENT=production
```

**Step 4:** Restart connector containers

**If using docker-compose-production.yml:**

```bash
docker-compose -f docker-compose-production.yml --env-file .env.production up -d
```

**If using standard docker-compose.yml with production config:**

```bash
# Link .env.production as .env
ln -sf .env.production .env
docker-compose up -d
```

### Verification Steps

**Step 1:** Check logs for absence of development warnings

```bash
docker-compose logs connector-a | grep "⚠️  DEVELOPMENT MODE"
# Expected: No output (production mode has no development warnings)
```

**Step 2:** Verify chain ID matches mainnet expectations

```bash
docker-compose logs connector-a | grep "Chain ID"
# Should NOT see "Chain ID mismatch" warnings
```

**Step 3:** Query RPC endpoint to confirm mainnet connection

```bash
# Verify Base mainnet (chainId 8453 = 0x2105)
curl https://mainnet.base.org \
  -X POST \
  -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}'
# Expected: {"jsonrpc":"2.0","id":1,"result":"0x2105"}
```

**Step 4:** Verify connector startup logs show production config

```bash
docker-compose logs connector-a | head -50
# Should see logs indicating production environment (no development warnings)
```

### Common Pitfalls

#### Pitfall 1: Using Development Private Key in Production

**Symptom:** Connector exits during startup with error

```
ConfigurationError: Cannot use development private key in production. Use secure key from KMS/HSM.
```

**Problem:** `BASE_PRIVATE_KEY` matches known Anvil development key:

```
0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
```

**Solution:** Generate production private key using secure method

```bash
# Generate new Ethereum private key
openssl rand -hex 32
# Output: a3c5f9e8d7b6a1...  (64 hex characters)

# Prefix with 0x for Ethereum format
BASE_PRIVATE_KEY=0xa3c5f9e8d7b6a1...
```

#### Pitfall 2: Chain ID Mismatch

**Symptom:** Warning log during startup

```
⚠️  Chain ID mismatch: config expects 8453, RPC returned 84532
```

**Problem:** `BASE_CHAIN_ID` doesn't match actual RPC endpoint chain

**Common causes:**

- `BASE_RPC_URL` points to Base Sepolia (`https://sepolia.base.org`)
- `BASE_CHAIN_ID` is set to Base mainnet (`8453`)

**Solution:** Verify RPC URL and chain ID match

```bash
# Base Sepolia: chainId 84532, url https://sepolia.base.org
# Base Mainnet: chainId 8453, url https://mainnet.base.org

# Update .env.production
BASE_RPC_URL=https://mainnet.base.org
BASE_CHAIN_ID=8453
```

#### Pitfall 3: Using Localhost RPC in Production

**Symptom:** Connector exits during startup with error

```
ConfigurationError: Cannot use localhost RPC in production. Use public mainnet endpoint.
```

**Problem:** `ENVIRONMENT=production` but `BASE_RPC_URL` contains `localhost` or `127.0.0.1`

**Solution:** Update RPC URLs to public mainnet endpoints

```bash
# Incorrect (development config)
BASE_RPC_URL=http://localhost:8545

# Correct (production config)
BASE_RPC_URL=https://mainnet.base.org
```

#### Pitfall 4: Forgetting to Update ENVIRONMENT Variable

**Symptom:** Connector uses development configuration in production deployment

**Problem:** `ENVIRONMENT` variable defaults to `development`, uses local blockchain endpoints

**Common causes:**

- Forgot to set `ENVIRONMENT=production` in .env.production
- Shell override from .bashrc or .zshrc (check `export | grep ENVIRONMENT`)

**Solution:** Verify ENVIRONMENT value in logs and configuration

```bash
# Check .env.production file
grep ENVIRONMENT .env.production
# Should output: ENVIRONMENT=production

# Check running container environment
docker-compose exec connector-a env | grep ENVIRONMENT
# Should output: ENVIRONMENT=production
```

## Troubleshooting Configuration Issues

### Issue: "Cannot use development private key in production"

**Full Error:**

```
ConfigurationError: Cannot use development private key in production. Use secure key from KMS/HSM.
```

**Symptom:** Connector exits during startup validation

**Problem:** `BASE_PRIVATE_KEY` matches known development key from Anvil Account #0:

```
0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
```

**Root Causes:**

1. Copied .env.dev to .env.production without updating private keys
2. Used example private key from documentation in production
3. Accidentally committed development key to production deployment

**Solution:**

1. **Generate secure production private key:**

```bash
# Ethereum private key (64 hex characters)
openssl rand -hex 32
# Prefix with 0x: BASE_PRIVATE_KEY=0x...
```

2. **Store in AWS Secrets Manager (recommended):**

```bash
aws secretsmanager create-secret \
  --name m2m/base/private-key \
  --secret-string "0x..."
```

3. **Update .env.production with secure key:**

```bash
BASE_PRIVATE_KEY=  # Load from KMS at runtime
```

4. **Restart connector:**

```bash
docker-compose up -d --force-recreate
```

### Issue: "Chain ID mismatch: config expects 8453, RPC returned 84532"

**Full Error:**

```
⚠️  Chain ID mismatch: config expects 8453, RPC returned 84532
⚠️  Verify BASE_RPC_URL points to correct network
```

**Symptom:** Warning log during startup (non-blocking)

**Problem:** Configured `BASE_CHAIN_ID` doesn't match actual RPC endpoint chain ID

**Common Scenarios:**

| Configured Chain ID | RPC URL                    | Actual Chain ID      | Issue                         |
| ------------------- | -------------------------- | -------------------- | ----------------------------- |
| 8453 (mainnet)      | `https://sepolia.base.org` | 84532 (Sepolia)      | Wrong RPC URL for mainnet     |
| 84532 (Sepolia)     | `https://mainnet.base.org` | 8453 (mainnet)       | Wrong RPC URL for testnet     |
| 8453 (mainnet)      | `http://anvil:8545`        | 84532 (Sepolia fork) | Development RPC in production |

**Solution:**

1. **Verify RPC endpoint chain ID:**

```bash
curl $BASE_RPC_URL \
  -X POST \
  -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}'
# Response: {"result":"0x2105"}  (0x2105 = 8453 in decimal)
```

2. **Update .env configuration to match:**

```bash
# Production: Base mainnet
BASE_RPC_URL=https://mainnet.base.org
BASE_CHAIN_ID=8453

# Staging: Base Sepolia testnet
BASE_RPC_URL=https://sepolia.base.org
BASE_CHAIN_ID=84532
```

3. **Restart connector:**

```bash
docker-compose restart connector-a
```

### Issue: "Cannot use localhost RPC in production"

**Full Error:**

```
ConfigurationError: Cannot use localhost RPC in production. Use public mainnet endpoint.
```

**Symptom:** Connector exits during startup validation

**Problem:** Production `ENVIRONMENT` but RPC URL contains `localhost` or `127.0.0.1`

**Solution:**

1. **Update RPC URLs in .env.production:**

```bash
# Incorrect (development)
BASE_RPC_URL=http://localhost:8545

# Correct (production)
BASE_RPC_URL=https://mainnet.base.org
```

2. **Restart connector:**

```bash
docker-compose up -d --force-recreate
```

### Issue: Connector Connects to Wrong Blockchain

**Symptom:** Transactions fail with "unknown account" or "insufficient funds"

**Problem:** `ENVIRONMENT` variable mismatch or wrong .env file loaded

**Debugging Steps:**

1. **Check active ENVIRONMENT value:**

```bash
docker-compose exec connector-a env | grep ENVIRONMENT
# Expected: ENVIRONMENT=production (or development/staging)
```

2. **Check docker-compose .env file loading:**

```bash
# Verify which .env file is loaded
docker-compose config | grep -A 5 "environment:"
```

3. **Check connector logs for configuration:**

```bash
docker-compose logs connector-a | grep "⚠️"
# Development mode shows warnings with active config
# Production mode has no warnings
```

**Solution:**

1. **Verify correct .env file is loaded:**

```bash
# Option 1: Use --env-file flag
docker-compose --env-file .env.production up -d

# Option 2: Symlink .env to .env.production
ln -sf .env.production .env
docker-compose up -d
```

2. **Verify ENVIRONMENT value in .env file:**

```bash
grep ENVIRONMENT .env.production
# Should output: ENVIRONMENT=production
```

3. **Restart connector:**

```bash
docker-compose down
docker-compose up -d
```

## Environment Variable Reference

### Complete Environment Variable List

| Variable                | Development Default | Staging Default            | Production Default         | Description            |
| ----------------------- | ------------------- | -------------------------- | -------------------------- | ---------------------- |
| `ENVIRONMENT`           | `development`       | `staging`                  | `production`               | Environment selector   |
| `BASE_ENABLED`          | `true`              | `true`                     | `true`                     | Enable Base blockchain |
| `BASE_RPC_URL`          | `http://anvil:8545` | `https://sepolia.base.org` | `https://mainnet.base.org` | Base RPC endpoint      |
| `BASE_CHAIN_ID`         | `84532`             | `84532`                    | `8453`                     | Base chain ID          |
| `BASE_PRIVATE_KEY`      | (Anvil #0)          | (test key)                 | (from KMS)                 | Private key            |
| `BASE_REGISTRY_ADDRESS` | (empty)             | (test deploy)              | (prod deploy)              | Contract address       |

## Related Documentation

- **[Local Blockchain Development Guide](./local-blockchain-development.md):** Detailed setup instructions for Anvil local node
- **[Story 7.1: Anvil Docker Service](../../docs/stories/7.1.story.md):** Base L2 local development infrastructure
- **[Story 7.3: Docker Compose Integration](../../docs/stories/7.3.story.md):** Full dev stack orchestration

## Additional Resources

### External Documentation

- **[Base Network Documentation](https://docs.base.org/):** Official Base L2 documentation
- **[Alchemy Quickstart](https://www.alchemy.com/overviews/base-rpc):** Base mainnet RPC provider
- **[Infura Base Support](https://docs.infura.io/networks/base):** Base mainnet RPC provider

### Monitoring and Alerting

**Recommended CloudWatch Alarms for Production:**

1. **RPC Endpoint Health:**
   - Metric: `RPCLatency` > 500ms (5-minute average)
   - Action: SNS notification to on-call engineer

2. **RPC Error Rate:**
   - Metric: `RPCErrors` > 1% of requests
   - Action: Page on-call, investigate RPC provider status

3. **Chain ID Validation:**
   - Metric: Log filter for "Chain ID mismatch"
   - Action: SNS notification (misconfiguration alert)

4. **Private Key Validation:**
   - Metric: Log filter for "development private key"
   - Action: Critical alert, stop deployment

### Support and Troubleshooting

If you encounter configuration issues not covered in this guide:

1. **Check connector logs for detailed error messages:**

   ```bash
   docker-compose logs connector-a | grep -i error
   ```

2. **Verify environment variable loading:**

   ```bash
   docker-compose config | grep -A 20 "environment:"
   ```

3. **Review recent changes to .env files:**

   ```bash
   git diff .env.production.example
   ```

4. **Consult related documentation** (links above)

5. **Open GitHub issue** with logs and configuration (redact secrets)
