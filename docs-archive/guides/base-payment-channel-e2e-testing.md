# Base Payment Channel E2E Testing (BLS → Admin API)

Complete end-to-end testing guide for Base payment channels from a Business Logic Service (BLS) perspective.

## Overview

This E2E test simulates **real-world production usage** where a BLS (like agent-society) makes HTTP requests to connector admin APIs to negotiate payment channels.

### Test Architecture

```
┌─────────────────────────────────────────────────────────────┐
│  BLS Test Suite (Jest)                                       │
│  - Makes HTTP POST/GET requests                             │
│  - Tests admin API endpoints                                │
│  - Simulates production BLS behavior                        │
└───────────────┬─────────────────────┬───────────────────────┘
                │                     │
        ┌───────▼─────────┐   ┌──────▼─────────┐
        │  Connector A    │   │  Connector B   │
        │  Admin API      │   │  Admin API     │
        │  :8081          │   │  :8082         │
        └───────┬─────────┘   └──────┬─────────┘
                │                     │
        ┌───────▼─────────┐   ┌──────▼─────────┐
        │ PaymentChannel  │   │ PaymentChannel │
        │     SDK         │   │      SDK       │
        └───────┬─────────┘   └──────┬─────────┘
                │                     │
                └──────────┬──────────┘
                           │
                  ┌────────▼────────┐
                  │  Anvil (Base)   │
                  │  :8545          │
                  └────────┬────────┘
                           │
                ┌──────────┴──────────┐
                │  Smart Contracts    │
                │  - TokenNetwork     │
                │  - Registry         │
                │  - MockERC20        │
                └─────────────────────┘
```

## Quick Start

```bash
# Option 1: Automated script (recommended)
./scripts/run-base-e2e-tests.sh

# Option 2: Using npm
E2E_TESTS=true npm run test:base-e2e --workspace=packages/connector

# Option 3: Manual
docker compose -f docker-compose-base-e2e-test.yml up -d
npm test --workspace=packages/connector -- base-payment-channel-e2e.test.ts
docker compose -f docker-compose-base-e2e-test.yml down -v
```

## What Gets Tested

### 1. **BLS → Admin API Integration**

Tests HTTP requests to connector admin endpoints (like agent-society would):

```typescript
// BLS opens a payment channel via HTTP POST
POST http://localhost:8081/admin/channels
{
  "peerId": "connector-b",
  "chain": "evm:base:8453",
  "token": "0x5FbDB2315678afecb367f032d93F642f64180aa3",
  "tokenNetwork": "0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0",
  "initialDeposit": "100000000000000000000",
  "settlementTimeout": 3600,
  "peerAddress": "0x70997970C51812dc3A010C7d01b50e0d17dc79C8"
}
```

### 2. **Complete Channel Lifecycle**

- ✅ **Open Channel**: POST /admin/channels
- ✅ **Get Channel Details**: GET /admin/channels/:channelId
- ✅ **List All Channels**: GET /admin/channels
- ✅ **Deposit Tokens**: POST /admin/channels/:channelId/deposit
- ✅ **Close Channel**: POST /admin/channels/:channelId/close
- ✅ **Verify State**: Check on-chain state matches API responses

### 3. **Error Handling**

- ✅ Invalid chain format rejection
- ✅ Invalid token address rejection
- ✅ Invalid deposit amount rejection
- ✅ Non-existent channel handling
- ✅ Duplicate channel detection

### 4. **Multi-Connector Scenarios**

- ✅ Two connectors opening channels independently
- ✅ Cross-connector state verification
- ✅ Independent admin API instances

## Services

### Connector A

- **Admin API**: http://localhost:8081
- **Health**: http://localhost:8080/health
- **BTP Port**: 4001
- **Address**: `0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266`

### Connector B

- **Admin API**: http://localhost:8082
- **Health**: http://localhost:8090/health
- **BTP Port**: 4002
- **Address**: `0x70997970C51812dc3A010C7d01b50e0d17dc79C8`

### Infrastructure

- **Anvil RPC**: http://localhost:8545
- **Token Faucet**: http://localhost:8546
- **TigerBeetle**: Internal (port 3000)

## Test Flow Example

```typescript
// 1. BLS opens channel from Connector A
const openResponse = await fetch('http://localhost:8081/admin/channels', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({
    peerId: 'connector-b',
    chain: 'evm:base:8453',
    token: '0x5FbDB2315678afecb367f032d93F642f64180aa3',
    // ... other params
  }),
});

const { channelId } = await openResponse.json();
// channelId: "0xabc123..."

// 2. BLS deposits tokens
await fetch(`http://localhost:8081/admin/channels/${channelId}/deposit`, {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ amount: '100000000000000000000' }),
});

// 3. BLS retrieves channel state
const stateResponse = await fetch(`http://localhost:8081/admin/channels/${channelId}`);
const state = await stateResponse.json();
// { channelId, status: 'open', deposit: '100000000000000000000', ... }

// 4. BLS closes channel cooperatively
await fetch(`http://localhost:8081/admin/channels/${channelId}/close`, {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ cooperative: true }),
});
```

## Prerequisites

### Required Software

- Docker Desktop or Docker Engine
- Docker Compose 2.x
- Node.js >= 22.11.0
- npm >= 10.0.0

### Environment Configuration

Create `.env.dev`:

```bash
# Base Sepolia RPC URL for forking
BASE_SEPOLIA_RPC_URL=https://sepolia.base.org

# Block number to fork from
FORK_BLOCK_NUMBER=20702367

# Enable E2E tests
E2E_TESTS=true
```

### Build Connector Image

The E2E test requires a built connector Docker image:

```bash
docker build -t connector/connector:latest .
```

## Running Tests

### Automated Script (Recommended)

```bash
# Run all E2E tests
./scripts/run-base-e2e-tests.sh

# Keep containers running for debugging
./scripts/run-base-e2e-tests.sh --no-cleanup

# Show service logs before tests
./scripts/run-base-e2e-tests.sh --logs

# Verbose test output
./scripts/run-base-e2e-tests.sh --verbose
```

### Manual Steps

```bash
# 1. Build connector image
docker build -t connector/connector:latest .

# 2. Start infrastructure
docker compose -f docker-compose-base-e2e-test.yml up -d

# 3. Wait for services (2-3 minutes on first run)
# Check status
docker compose -f docker-compose-base-e2e-test.yml ps

# Wait for health
curl http://localhost:8080/health  # Connector A
curl http://localhost:8090/health  # Connector B

# 4. Run tests
E2E_TESTS=true npm test --workspace=packages/connector -- base-payment-channel-e2e.test.ts

# 5. Cleanup
docker compose -f docker-compose-base-e2e-test.yml down -v
```

## Debugging

### View Service Logs

```bash
# All services
docker compose -f docker-compose-base-e2e-test.yml logs -f

# Specific service
docker compose -f docker-compose-base-e2e-test.yml logs -f connector_a
docker compose -f docker-compose-base-e2e-test.yml logs -f connector_b
docker compose -f docker-compose-base-e2e-test.yml logs -f anvil_base_e2e
```

### Test Admin API Manually

```bash
# Open a channel
curl -X POST http://localhost:8081/admin/channels \
  -H "Content-Type: application/json" \
  -d '{
    "peerId": "connector-b",
    "chain": "evm:base:8453",
    "token": "0x5FbDB2315678afecb367f032d93F642f64180aa3",
    "tokenNetwork": "0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0",
    "initialDeposit": "0",
    "settlementTimeout": 3600,
    "peerAddress": "0x70997970C51812dc3A010C7d01b50e0d17dc79C8"
  }'

# List channels
curl http://localhost:8081/admin/channels

# Get channel details
curl http://localhost:8081/admin/channels/0x...

# Deposit tokens
curl -X POST http://localhost:8081/admin/channels/0x.../deposit \
  -H "Content-Type: application/json" \
  -d '{"amount": "100000000000000000000"}'
```

### Check On-Chain State

```bash
# Get block number
curl -X POST http://localhost:8545 \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}'

# Get token balance
docker exec anvil_base_e2e cast call \
  0x5FbDB2315678afecb367f032d93F642f64180aa3 \
  "balanceOf(address)(uint256)" \
  0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266 \
  --rpc-url http://localhost:8545
```

## Differences from SDK-Level Tests

| Aspect          | SDK Test           | E2E Test                           |
| --------------- | ------------------ | ---------------------------------- |
| **Entry Point** | Direct SDK calls   | HTTP requests to Admin API         |
| **Simulates**   | Internal SDK logic | Real BLS usage                     |
| **Services**    | Anvil only         | Anvil + 2 Connectors + TigerBeetle |
| **Scope**       | PaymentChannelSDK  | Full connector stack               |
| **Runtime**     | ~1-2 min           | ~3-5 min                           |
| **Complexity**  | Low                | High                               |

### SDK Test (Low-Level)

```typescript
const sdk = new PaymentChannelSDK(...);
await sdk.openChannel(...);
```

### E2E Test (Production-Like)

```typescript
const response = await fetch('http://localhost:8081/admin/channels', {
  method: 'POST',
  body: JSON.stringify({ peerId, chain, token, ... })
});
```

## Troubleshooting

### Connector fails to start

**Check build**:

```bash
docker build -t connector/connector:latest .
```

**Check logs**:

```bash
docker compose -f docker-compose-base-e2e-test.yml logs connector_a
```

**Common issues**:

- Missing environment variables
- TigerBeetle not initialized
- Port conflicts

### Tests timeout

**Increase timeout**:

```typescript
jest.setTimeout(600000); // Already set to 10 minutes
```

**Check service health**:

```bash
curl http://localhost:8080/health
curl http://localhost:8090/health
```

### Channel operations fail

**Check contract deployment**:

```bash
docker compose -f docker-compose-base-e2e-test.yml logs contract_deployer_e2e
```

**Verify addresses**:

- Token: `0x5FbDB2315678afecb367f032d93F642f64180aa3`
- Registry: `0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512`
- TokenNetwork: `0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0`

## Performance

Expected timings (M1 MacBook Pro, 16GB RAM):

- **Docker startup**: 2-3 minutes (first run)
- **Docker startup**: 30-60 seconds (subsequent runs)
- **Full test suite**: 3-5 minutes
- **Individual test**: 5-10 seconds

## CI/CD Integration

```yaml
name: Base E2E Tests

on: [push, pull_request]

jobs:
  base-e2e:
    runs-on: ubuntu-latest
    timeout-minutes: 20

    steps:
      - uses: actions/checkout@v3

      - name: Setup Node.js
        uses: actions/setup-node@v3
        with:
          node-version: '22'

      - name: Install dependencies
        run: npm ci

      - name: Build connector image
        run: docker build -t connector/connector:latest .

      - name: Run E2E tests
        run: ./scripts/run-base-e2e-tests.sh
        env:
          E2E_TESTS: true

      - name: Upload logs on failure
        if: failure()
        uses: actions/upload-artifact@v3
        with:
          name: e2e-logs
          path: |
            docker-compose-base-e2e-test.yml
```

## Related Documentation

- [SDK-Level Tests](./base-payment-channel-testing.md) - Low-level PaymentChannelSDK tests
- [Quick Start](./quick-start-base-tests.md) - Quick SDK test reference
- [Admin API Documentation](../../packages/connector/src/http/admin-api.ts) - Admin API implementation
- [PaymentChannelSDK](../../packages/connector/src/settlement/payment-channel-sdk.ts) - SDK source code

## Contributing

When modifying the E2E test:

1. **Update both test files**:
   - SDK test: `base-payment-channel.test.ts`
   - E2E test: `base-payment-channel-e2e.test.ts`

2. **Test locally**:

   ```bash
   ./scripts/run-base-e2e-tests.sh
   ```

3. **Update documentation**:
   - This file
   - `BASE_PAYMENT_CHANNEL_TESTS.md`

4. **Verify CI passes**:
   - Push and check GitHub Actions

## License

MIT
