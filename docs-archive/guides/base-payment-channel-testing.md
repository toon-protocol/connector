# Base Payment Channel Integration Testing

This guide explains how to run integration tests for EVM payment channels on Base using a local Anvil fork.

## Overview

The Base payment channel integration tests verify the complete lifecycle of payment channels including:

- Opening channels between participants
- Depositing tokens into channels
- Creating and verifying EIP-712 balance proofs
- Cooperative settlement
- Dispute resolution (close + settle after challenge period)

## Architecture

```
┌─────────────────┐
│  Test Suite     │
│  (Jest)         │
└────────┬────────┘
         │
         ├─────────────────┐
         │                 │
    ┌────▼─────┐     ┌─────▼──────┐
    │  Anvil   │     │   Faucet   │
    │  (Base   │     │  (Token    │
    │  Fork)   │     │  Service)  │
    └──────────┘     └────────────┘
         │
    ┌────▼─────────────────┐
    │  Smart Contracts     │
    │  - TokenNetwork      │
    │  - TokenNetworkReg   │
    │  - MockERC20        │
    └──────────────────────┘
```

## Prerequisites

### Required Software

- Docker Desktop or Docker Engine
- Docker Compose 2.x
- Node.js >= 22.11.0
- npm >= 10.0.0

### Environment Configuration

Create a `.env.dev` file (or copy from `.env.dev.example`):

```bash
# Base Sepolia RPC URL for forking
BASE_SEPOLIA_RPC_URL=https://sepolia.base.org

# Block number to fork from (update periodically)
FORK_BLOCK_NUMBER=20702367

# Enable E2E tests
E2E_TESTS=true
```

#### Alternative RPC Endpoints

If you encounter rate limits with the public Base Sepolia RPC:

**Alchemy** (free tier available):

```bash
BASE_SEPOLIA_RPC_URL=https://base-sepolia.g.alchemy.com/v2/YOUR_API_KEY
```

**Tenderly**:

```bash
BASE_SEPOLIA_RPC_URL=https://base-sepolia.gateway.tenderly.co
```

**Infura**:

```bash
BASE_SEPOLIA_RPC_URL=https://base-sepolia.infura.io/v3/YOUR_PROJECT_ID
```

## Quick Start

### 1. Start Test Infrastructure

From the repository root:

```bash
# Start Anvil, faucet, and deploy contracts
docker compose -f docker-compose-base-test.yml up -d

# View logs
docker compose -f docker-compose-base-test.yml logs -f

# Check service health
docker compose -f docker-compose-base-test.yml ps
```

Expected output:

```
NAME                   STATUS              PORTS
anvil_base_local       Up (healthy)        0.0.0.0:8545->8545/tcp
contract_deployer      Up
token_faucet           Up (healthy)        0.0.0.0:8546->8546/tcp
```

### 2. Run Integration Tests

```bash
# Run all Base payment channel tests
npm test --workspace=packages/connector -- base-payment-channel.test.ts

# Run with verbose output
TEST_LOG_LEVEL=debug npm test --workspace=packages/connector -- base-payment-channel.test.ts

# Run specific test suite
npm test --workspace=packages/connector -- base-payment-channel.test.ts -t "Channel Lifecycle"
```

### 3. Cleanup

```bash
# Stop and remove all containers and volumes
docker compose -f docker-compose-base-test.yml down -v
```

## Services

### Anvil (Base Sepolia Fork)

- **Port**: 8545
- **Chain ID**: 84532 (Base Sepolia)
- **Accounts**: 10 pre-funded accounts (10,000 ETH each)
- **Block Time**: 1 second
- **Fork**: Base Sepolia at specified block number

**RPC Endpoint**: `http://localhost:8545`

#### Test Accounts

| Account    | Address                                      | Private Key                                                          |
| ---------- | -------------------------------------------- | -------------------------------------------------------------------- |
| Account #0 | `0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266` | `0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80` |
| Account #1 | `0x70997970C51812dc3A010C7d01b50e0d17dc79C8` | `0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d` |

⚠️ **WARNING**: These are publicly known test keys. **NEVER** use them on mainnet or with real funds.

### Token Faucet

- **Port**: 8546
- **Amount**: 1,000 tokens per request
- **Token**: MockERC20 (deployed at `0x5FbDB2315678afecb367f032d93F642f64180aa3`)

**Usage**:

```bash
# Fund an account with test tokens
curl -X POST http://localhost:8546/fund/0x70997970C51812dc3A010C7d01b50e0d17dc79C8

# Response
{
  "success": true,
  "txHash": "0x...",
  "amount": "1000000000000000000000",
  "recipient": "0x70997970C51812dc3A010C7d01b50e0d17dc79C8"
}
```

### Deployed Contracts

Contracts are automatically deployed on startup using Foundry's deterministic deployment.

| Contract               | Address                                      |
| ---------------------- | -------------------------------------------- |
| MockERC20 (Test Token) | `0x5FbDB2315678afecb367f032d93F642f64180aa3` |
| TokenNetworkRegistry   | `0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512` |

## Test Structure

### Test Suites

1. **Channel Lifecycle**
   - Opening channels
   - Depositing tokens
   - Creating balance proofs
   - Cooperative settlement

2. **Channel Dispute Resolution**
   - Unilateral channel close
   - Challenge period
   - Settlement after timeout

3. **SDK Functionality**
   - TokenNetwork address lookup
   - Channel enumeration

### Example Test Flow

```typescript
// 1. Open channel
const { channelId } = await sdk0.openChannel(
  participant2Address,
  tokenAddress,
  settlementTimeout,
  0n
);

// 2. Deposit tokens
await sdk0.deposit(channelId, tokenAddress, depositAmount);

// 3. Create balance proof
const signature = await sdk0.signBalanceProof(
  channelId,
  nonce,
  transferredAmount,
  0n,
  ethers.ZeroHash
);

// 4. Verify balance proof
const isValid = await sdk1.verifyBalanceProof(balanceProof, signature, participant1Address);

// 5. Cooperatively settle
await sdk0.cooperativeSettle(channelId, tokenAddress, proof0, sig0, proof1, sig1);
```

## Troubleshooting

### Docker Issues

**Container fails to start**:

```bash
# Check logs
docker compose -f docker-compose-base-test.yml logs anvil_base_local

# Restart services
docker compose -f docker-compose-base-test.yml restart
```

**Port conflicts**:

```bash
# Check if ports 8545/8546 are in use
lsof -i :8545
lsof -i :8546

# Stop conflicting services or modify docker-compose-base-test.yml ports
```

### Fork Issues

**Fork download fails**:

- Check your `BASE_SEPOLIA_RPC_URL` is accessible
- Try an alternative RPC provider (Alchemy, Tenderly, Infura)
- Verify network connectivity

**Fork is outdated**:

```bash
# Get latest block number
curl https://sepolia.base.org \
  -X POST \
  -H "Content-Type: application/json" \
  --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}'

# Update FORK_BLOCK_NUMBER in .env.dev
```

### Test Failures

**Contract deployment failed**:

```bash
# Check deployer logs
docker compose -f docker-compose-base-test.yml logs contract_deployer

# Restart deployment
docker compose -f docker-compose-base-test.yml restart contract_deployer
```

**Faucet not responding**:

```bash
# Check faucet logs
docker compose -f docker-compose-base-test.yml logs token_faucet

# Test faucet manually
curl -X POST http://localhost:8546/fund/0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266
```

**Balance proof verification fails**:

- Ensure both participants are using the same `channelId`
- Verify the correct `nonce` is being used (must be sequential)
- Check that signatures are created with the correct private key

### Common Errors

**"No TokenNetwork found for token"**:

- TokenNetworkRegistry may not be deployed
- Check contract deployment logs
- Verify `REGISTRY_ADDRESS` matches deployed contract

**"Challenge period not expired"**:

- This is expected behavior for dispute settlement
- Tests include a wait period for the challenge timeout
- Verify `settlementTimeout` in test matches on-chain value

## Advanced Usage

### Manual Testing with cast

```bash
# Get token balance
docker exec anvil_base_local cast call \
  0x5FbDB2315678afecb367f032d93F642f64180aa3 \
  "balanceOf(address)(uint256)" \
  0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266 \
  --rpc-url http://localhost:8545

# Check channel state
docker exec anvil_base_local cast call \
  <TOKEN_NETWORK_ADDRESS> \
  "channels(bytes32)" \
  <CHANNEL_ID> \
  --rpc-url http://localhost:8545
```

### Debugging with Hardhat Console

```bash
# Attach to Anvil RPC
npx hardhat console --network localhost

# In console
const provider = ethers.provider;
const blockNumber = await provider.getBlockNumber();
console.log("Current block:", blockNumber);
```

## Performance

Typical test execution times:

- Docker startup: 30-60 seconds (first run)
- Contract deployment: 5-10 seconds
- Full test suite: 60-120 seconds
- Individual test: 2-5 seconds

Total end-to-end time: ~2-3 minutes

## CI/CD Integration

Example GitHub Actions workflow:

```yaml
name: Base Payment Channel Tests

on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3

      - name: Setup Node.js
        uses: actions/setup-node@v3
        with:
          node-version: '22'

      - name: Install dependencies
        run: npm ci

      - name: Start test infrastructure
        run: docker compose -f docker-compose-base-test.yml up -d

      - name: Wait for services
        run: |
          sleep 60
          docker compose -f docker-compose-base-test.yml ps

      - name: Run tests
        run: npm test --workspace=packages/connector -- base-payment-channel.test.ts
        env:
          E2E_TESTS: true

      - name: Cleanup
        if: always()
        run: docker compose -f docker-compose-base-test.yml down -v
```

## References

- [PaymentChannelSDK Documentation](../../packages/connector/src/settlement/payment-channel-sdk.ts)
- [TokenNetwork Contract](../../packages/contracts/src/TokenNetwork.sol)
- [Foundry Anvil Documentation](https://book.getfoundry.sh/anvil/)
- [Base Sepolia Testnet](https://docs.base.org/network-information)
- [EIP-712 Typed Data](https://eips.ethereum.org/EIPS/eip-712)
