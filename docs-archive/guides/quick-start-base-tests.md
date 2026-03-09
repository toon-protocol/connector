# Quick Start: Base Payment Channel Tests

Run the complete Base payment channel integration test suite with a single command.

## Prerequisites

- Docker Desktop or Docker Engine running
- Node.js >= 22.11.0
- npm >= 10.0.0

## One-Command Test Run

```bash
# From repository root
./scripts/run-base-channel-tests.sh
```

This script will:

1. ✅ Start Anvil (Base Sepolia fork)
2. ✅ Deploy smart contracts
3. ✅ Start token faucet service
4. ✅ Run all integration tests
5. ✅ Clean up Docker containers

## Options

### Keep containers running

```bash
./scripts/run-base-channel-tests.sh --no-cleanup
```

After tests complete, containers remain running for manual inspection:

```bash
# View logs
docker-compose -f docker-compose-base-test.yml logs -f

# Check token balance
docker exec anvil_base_local cast call \
  0x5FbDB2315678afecb367f032d93F642f64180aa3 \
  "balanceOf(address)(uint256)" \
  0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266 \
  --rpc-url http://localhost:8545

# Stop manually when done
docker-compose -f docker-compose-base-test.yml down -v
```

### Verbose test output

```bash
./scripts/run-base-channel-tests.sh --verbose
```

Shows detailed logging including:

- PaymentChannelSDK operations
- Transaction hashes
- Balance updates
- Event emissions

## Manual Steps (Alternative)

If you prefer to run each step manually:

### 1. Start Infrastructure

```bash
docker-compose -f docker-compose-base-test.yml up -d
```

### 2. Wait for Services

```bash
# Check status
docker-compose -f docker-compose-base-test.yml ps

# Should show:
# anvil_base_local       Up (healthy)
# contract_deployer      Up
# token_faucet           Up (healthy)
```

### 3. Run Tests

```bash
# All tests
npm test --workspace=packages/connector -- base-payment-channel.test.ts

# Specific test suite
npm test --workspace=packages/connector -- base-payment-channel.test.ts -t "Channel Lifecycle"

# With debug output
TEST_LOG_LEVEL=debug npm test --workspace=packages/connector -- base-payment-channel.test.ts
```

### 4. Cleanup

```bash
docker-compose -f docker-compose-base-test.yml down -v
```

## Expected Output

```
🚀 Base Payment Channel Integration Tests
==========================================

🧹 Stopping any existing containers...

🐳 Starting Docker Compose services...

⏳ Waiting for Anvil to be ready...
   This may take 30-60 seconds for first run (downloading fork)...
✅ Anvil is ready!

⏳ Waiting for contract deployment...

📄 Deployment logs:
  Deployer address: 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266
  AgentToken deployed to: 0x5FbDB2315678afecb367f032d93F642f64180aa3
  TokenNetworkRegistry deployed to: 0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512
  TokenNetwork created at: 0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0

⏳ Waiting for token faucet...
✅ Token faucet is ready!

📊 Service status:
NAME                   STATUS              PORTS
anvil_base_local       Up (healthy)        0.0.0.0:8545->8545/tcp
contract_deployer      Up
token_faucet           Up (healthy)        0.0.0.0:8546->8546/tcp

🧪 Running integration tests...
==========================================

PASS packages/connector/test/integration/base-payment-channel.test.ts
  Base Payment Channel Integration Tests
    Channel Lifecycle
      ✓ should open a payment channel between two parties (1234ms)
      ✓ should deposit tokens into the channel (892ms)
      ✓ should create and verify balance proofs (456ms)
      ✓ should cooperatively settle a channel (1123ms)
    Channel Dispute Resolution
      ✓ should open and deposit for dispute test (891ms)
      ✓ should close channel with balance proof (678ms)
      ✓ should wait for challenge period and settle (61234ms)
    SDK Functionality
      ✓ should get TokenNetwork address for a token (234ms)
      ✓ should list channels for an account (345ms)

==========================================
✅ All tests completed successfully!
==========================================

🧹 Cleaning up Docker containers...
✅ Cleanup complete
```

## Troubleshooting

### Fork download is slow

First run downloads the Base Sepolia fork state (can take 30-60 seconds). Subsequent runs use cached Docker volumes.

To use a different RPC endpoint (potentially faster):

```bash
# Edit .env.dev
BASE_SEPOLIA_RPC_URL=https://base-sepolia.g.alchemy.com/v2/YOUR_API_KEY
```

### Port conflicts

If ports 8545 or 8546 are already in use:

```bash
# Find conflicting process
lsof -i :8545
lsof -i :8546

# Stop the process or modify docker-compose-base-test.yml to use different ports
```

### Docker daemon not running

```bash
# Start Docker Desktop
# OR
sudo systemctl start docker  # Linux
```

## Next Steps

- [Full Testing Guide](./base-payment-channel-testing.md) - Detailed documentation
- [PaymentChannelSDK](../../packages/connector/src/settlement/payment-channel-sdk.ts) - SDK source code
- [Smart Contracts](../../packages/contracts/src/) - TokenNetwork contracts
