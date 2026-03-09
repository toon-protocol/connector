# Local Blockchain Node Setup Guide for Payment Channel Testing

# M2M Project - Base L2 (EVM) and XRP Ledger Local Development

**Document Version:** 1.0
**Last Updated:** 2025-01-02
**Status:** Research Complete

---

## Executive Summary

This guide provides comprehensive instructions for setting up local Base L2 (EVM) and XRP Ledger nodes for payment channel development and testing in the M2M project. Based on research conducted in January 2025, this document evaluates multiple approaches and provides specific recommendations with actionable configuration examples.

### Key Recommendations

**For Base L2 (EVM) Development:**

- **Primary Recommendation:** Use **Anvil (Foundry)** for local development
- **Approach:** Fork Base Sepolia testnet for realistic testing
- **Why:** Instant blocks, zero gas costs, 10 pre-funded accounts, fast iteration
- **Alternative:** Hardhat Network with Base Sepolia fork

**For XRP Ledger Development:**

- **Primary Recommendation:** Use **rippled standalone mode** via Docker
- **Approach:** Docker container with manual ledger advancement
- **Why:** Official implementation, payment channel support, complete control
- **No Alternative:** rippled is the canonical XRPL implementation

### Quick Start Summary

| Chain   | Tool   | Command                                                    | RPC Endpoint          | Setup Time |
| ------- | ------ | ---------------------------------------------------------- | --------------------- | ---------- |
| Base L2 | Anvil  | `anvil --fork-url https://sepolia.base.org`                | http://localhost:8545 | 30 seconds |
| XRPL    | Docker | `docker run -p 5005:5005 xrpllabsofficial/xrpld:latest -a` | http://localhost:5005 | 1 minute   |

### Estimated Setup Time and Learning Curve

- **Base L2 with Anvil:** 1-2 hours (including Foundry installation and testing)
- **XRPL with Docker:** 2-3 hours (including Docker setup and payment channel testing)
- **Combined Setup:** 3-5 hours (both chains with Docker Compose integration)
- **Learning Curve:** Low for developers familiar with Ethereum/blockchain development

### Critical Gotchas and Limitations

**Base L2 (Anvil):**

- Forking requires external RPC endpoint (can hit rate limits on free tiers)
- Fork state diverges from mainnet over time - use `--fork-block-number` to pin
- OP Stack-specific features require `--optimism` flag
- No persistence by default - use `--dump-state`/`--load-state` for stateful testing

**XRPL (rippled standalone):**

- Ledgers don't advance automatically - must call `ledger_accept` after each transaction
- No consensus process - all transactions succeed immediately
- Testnet resets periodically (data loss risk)
- Payment channel testing requires understanding of claim/signature mechanics

---

## Table of Contents

1. [Base L2 Local Setup](#base-l2-local-setup)
   - 1.1 [Anvil (Foundry) - Recommended](#11-anvil-foundry---recommended)
   - 1.2 [Hardhat Network - Alternative](#12-hardhat-network---alternative)
   - 1.3 [Full Base Node - Production Only](#13-full-base-node---production-only)
2. [XRP Ledger Local Setup](#xrp-ledger-local-setup)
   - 2.1 [rippled Standalone Mode](#21-rippled-standalone-mode)
   - 2.2 [Payment Channel Configuration](#22-payment-channel-configuration)
3. [Docker Compose Integration](#docker-compose-integration)
4. [Testing Payment Channels](#testing-payment-channels)
   - 4.1 [EVM Payment Channel Testing](#41-evm-payment-channel-testing)
   - 4.2 [XRPL Payment Channel Testing](#42-xrpl-payment-channel-testing)
5. [Development Workflow Integration](#development-workflow-integration)
6. [Performance and Resource Requirements](#performance-and-resource-requirements)
7. [Troubleshooting and Debugging](#troubleshooting-and-debugging)
8. [References and Additional Resources](#references-and-additional-resources)

---

## 1. Base L2 Local Setup

### 1.1 Anvil (Foundry) - Recommended

**Anvil** is a fast, local Ethereum development node built with Rust, part of the Foundry toolkit. It's the recommended approach for Base L2 local development due to speed, simplicity, and zero-cost iteration.

#### Why Anvil?

- **Instant block mining:** No waiting for block times during development
- **Pre-funded accounts:** 10 accounts with 10,000 ETH each
- **Fork any network:** Fork Base Sepolia or Base Mainnet for realistic testing
- **Fast iteration:** Restart in seconds, no blockchain sync required
- **Performance:** 1,200+ tests/second (vs Hardhat's 450 tests/second)
- **Built-in tooling:** Integrates with `forge` (testing) and `cast` (blockchain interactions)

#### Installation

```bash
# Install Foundry (includes anvil, forge, cast, chisel)
curl -L https://foundry.paradigm.xyz | bash
foundryup

# Verify installation
anvil --version
```

#### Basic Usage - Local Devnet

```bash
# Start local Anvil node (instant blocks, no forking)
anvil

# Output shows:
# - 10 pre-funded accounts with private keys
# - RPC endpoint: http://127.0.0.1:8545
# - Chain ID: 31337
```

**Example output:**

```
Available Accounts
==================
(0) 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266 (10000.000000000000000000 ETH)
(1) 0x70997970C51812dc3A010C7d01b50e0d17dc79C8 (10000.000000000000000000 ETH)
...

Private Keys
==================
(0) 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
(1) 0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d
...

Listening on 127.0.0.1:8545
```

**WARNING:** These accounts are public knowledge. Never use them on mainnet or any real blockchain.

#### Fork Base Sepolia Testnet

```bash
# Fork Base Sepolia for realistic testing
anvil --fork-url https://sepolia.base.org

# With specific options
anvil \
  --fork-url https://sepolia.base.org \
  --fork-block-number 20702367 \
  --chain-id 84532 \
  --optimism
```

**Configuration Options:**

| Option                    | Purpose                                | Example                    |
| ------------------------- | -------------------------------------- | -------------------------- |
| `--fork-url <RPC>`        | Fork from external RPC                 | `https://sepolia.base.org` |
| `--fork-block-number <N>` | Pin to specific block (prevents drift) | `20702367`                 |
| `--chain-id <ID>`         | Override chain ID                      | `84532` (Base Sepolia)     |
| `--optimism`              | Enable Optimism/OP Stack features      | Required for Base L2       |
| `--port <PORT>`           | Change RPC port                        | `8545` (default)           |
| `--host <HOST>`           | Bind to specific host                  | `0.0.0.0` (all interfaces) |
| `--dump-state <FILE>`     | Save state on exit                     | `state.json`               |
| `--load-state <FILE>`     | Restore previous state                 | `state.json`               |

#### Fork Base Mainnet

```bash
# Fork Base Mainnet (use for production contract testing)
anvil --fork-url https://mainnet.base.org --chain-id 8453

# Alternative RPC endpoints (to avoid rate limits)
anvil --fork-url https://base-rpc.publicnode.com
anvil --fork-url https://base.gateway.tenderly.co
```

**Note:** Free public RPC endpoints are rate-limited. For heavy development, use:

- **Alchemy:** https://www.alchemy.com/rpc/base
- **QuickNode:** https://www.quicknode.com/
- **Infura:** (Base support coming soon)

#### Stateful Testing with Snapshots

```bash
# Start Anvil with state persistence
anvil \
  --fork-url https://sepolia.base.org \
  --fork-block-number 20702367 \
  --dump-state ~/anvil-state.json

# Later, restore from saved state
anvil --load-state ~/anvil-state.json
```

#### Configuration File (foundry.toml)

```toml
# foundry.toml
[profile.default]
src = "src"
out = "out"
libs = ["lib"]
solc_version = "0.8.20"

[rpc_endpoints]
base_sepolia = "https://sepolia.base.org"
base_mainnet = "https://mainnet.base.org"
local = "http://localhost:8545"

[etherscan]
base_sepolia = { key = "${ETHERSCAN_API_KEY}", url = "https://api-sepolia.basescan.org/api" }
base_mainnet = { key = "${ETHERSCAN_API_KEY}", url = "https://api.basescan.org/api" }
```

#### Deploy Smart Contract with Forge

```bash
# Deploy to local Anvil
forge script script/Deploy.s.sol --rpc-url http://localhost:8545 --broadcast

# Deploy to Base Sepolia
forge script script/Deploy.s.sol --rpc-url base_sepolia --broadcast --verify

# Deploy to Base Mainnet (production)
forge script script/Deploy.s.sol --rpc-url base_mainnet --broadcast --verify
```

#### Interacting with Contracts (cast)

```bash
# Check balance
cast balance 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266

# Send transaction
cast send <CONTRACT> "functionName(uint256)" 123 --rpc-url http://localhost:8545

# Call view function
cast call <CONTRACT> "getValue()(uint256)" --rpc-url http://localhost:8545

# Get block number
cast block-number --rpc-url http://localhost:8545
```

---

### 1.2 Hardhat Network - Alternative

**Hardhat** is a popular Ethereum development environment with built-in network emulation. While Anvil is recommended for speed, Hardhat is a viable alternative if your team prefers JavaScript/TypeScript-based tooling.

#### Why Hardhat?

- **JavaScript/TypeScript native:** Familiar for web developers
- **Rich plugin ecosystem:** Extensive community plugins
- **Mainnet forking:** Fork any EVM network for testing
- **Debugging tools:** Built-in console.log in Solidity
- **Established ecosystem:** Mature tooling and documentation

#### Installation

```bash
# Initialize Node.js project
npm init -y
npm install --save-dev hardhat

# Initialize Hardhat project
npx hardhat init
```

#### Configuration for Base L2

```javascript
// hardhat.config.js
require('@nomiclabs/hardhat-waffle');
require('@nomiclabs/hardhat-ethers');
require('dotenv').config();

module.exports = {
  solidity: '0.8.20',
  networks: {
    hardhat: {
      // Local Hardhat Network (no forking)
      chainId: 31337,
    },
    baseFork: {
      // Fork Base Sepolia locally
      url: 'http://127.0.0.1:8545',
      forking: {
        url: process.env.BASE_SEPOLIA_RPC_URL || 'https://sepolia.base.org',
        blockNumber: 20702367, // Optional: pin to specific block
        enabled: true,
      },
    },
    base_sepolia: {
      url: process.env.BASE_SEPOLIA_RPC_URL || 'https://sepolia.base.org',
      accounts: [process.env.PRIVATE_KEY],
      chainId: 84532,
    },
    base_mainnet: {
      url: process.env.BASE_MAINNET_RPC_URL || 'https://mainnet.base.org',
      accounts: [process.env.PRIVATE_KEY],
      chainId: 8453,
    },
  },
};
```

#### Running Hardhat Network

```bash
# Start Hardhat Network (ephemeral, in-memory)
npx hardhat node

# Start with Base Sepolia fork
npx hardhat node --fork https://sepolia.base.org

# Deploy to local Hardhat Network
npx hardhat run scripts/deploy.js --network localhost

# Deploy to Base Sepolia
npx hardhat run scripts/deploy.js --network base_sepolia
```

#### Example Deployment Script

```javascript
// scripts/deploy.js
const hre = require('hardhat');

async function main() {
  const [deployer] = await hre.ethers.getSigners();
  console.log('Deploying with account:', deployer.address);

  const PaymentChannel = await hre.ethers.getContractFactory('PaymentChannel');
  const channel = await PaymentChannel.deploy();
  await channel.deployed();

  console.log('PaymentChannel deployed to:', channel.address);
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
```

#### Running Tests

```bash
# Run tests on local Hardhat Network
npx hardhat test

# Run tests on Base Sepolia fork
npx hardhat test --network baseFork
```

---

### 1.3 Full Base Node - Production Only

**Running a full Base node is NOT recommended for local development** due to hardware requirements, sync time, and complexity. Use Anvil or Hardhat for development.

#### When to Run a Full Node

- **Production RPC endpoint:** Serving public RPC requests
- **Data indexing:** Building block explorers or analytics
- **Archival queries:** Accessing historical state beyond standard retention

#### Hardware Requirements

**Production Node:**

- **CPU:** Intel i7 or AMD Ryzen 7 (8+ cores, 16+ threads)
- **RAM:** 16 GB minimum, 32 GB recommended
- **Storage:** 2+ TB NVMe SSD (10,000+ IOPS sustained)
- **Network:** 1+ Mbps upload, stable connection
- **Do NOT use:** AWS EBS (latency too high for reliable sync)

#### Prerequisites

- **L1 Ethereum RPC:** Access to synced Ethereum L1 execution client (Geth, Besu, Erigon)
- **L1 Beacon API:** Access to Ethereum consensus client (Lighthouse, Prysm, Teku)

**Options for L1 endpoints:**

- Self-hosted Ethereum node
- Alchemy, Infura, or QuickNode (paid)

#### Setup with Docker

```bash
# Clone Base node repository
git clone https://github.com/base/node.git
cd node

# Configure environment variables
cat > .env <<EOF
OP_NODE_L1_ETH_RPC=https://your-l1-rpc-endpoint
OP_NODE_L1_BEACON=https://your-l1-beacon-endpoint
OP_NODE_L1_BEACON_ARCHIVER=https://your-l1-beacon-archiver
EOF

# Start Base node (mainnet)
docker compose up --build

# Start Base Sepolia node (testnet)
NETWORK_ENV=.env.sepolia docker compose up --build
```

#### Sync Time Estimates

- **Without snapshot:** 1-3 days (depending on hardware and L1 RPC speed)
- **With snapshot:** 4-8 hours (recommended - snapshots updated daily)

**Using snapshots:**

```bash
# Download Base mainnet snapshot
wget https://base-snapshots.example.com/latest.tar.gz

# Extract to data directory
tar -xzf latest.tar.gz -C ./base-data
```

#### Why NOT for Development

- **Sync time:** Hours to days before usable
- **Storage:** Terabytes of disk space
- **Complexity:** L1 dependencies, networking, monitoring
- **Cost:** Hardware, electricity, maintenance
- **Overkill:** Local development doesn't need full node security/decentralization

**Use Anvil instead:** Instant startup, zero sync, free testing.

---

## 2. XRP Ledger Local Setup

### 2.1 rippled Standalone Mode

**rippled** is the reference implementation of the XRP Ledger protocol. **Standalone mode** is specifically designed for local development and testing without connecting to the live XRPL network.

#### Why Standalone Mode?

- **Offline operation:** No network peers, complete control
- **Manual ledger advancement:** Test transaction sequences without waiting
- **Instant transactions:** No consensus delay
- **Payment channel support:** Full XRPL payment channel implementation
- **Reset capability:** Clean slate for each test run

#### System Requirements

**Development/Testing:**

- **CPU:** 64-bit x86_64, 4+ cores
- **RAM:** 4-8 GB (standalone mode is lightweight)
- **Disk:** 10 GB minimum (no historical ledgers)
- **OS:** Linux, macOS, Windows (via Docker)

**Production Node (for reference only):**

- **CPU:** 3+ GHz, 8+ cores
- **RAM:** 16+ GB
- **Disk:** 50+ GB SSD/NVMe (10,000+ IOPS sustained)

#### Installation via Docker (Recommended)

**Option 1: Official rippled Image**

```bash
# Run rippled in standalone mode
docker run -d \
  --name rippled_standalone \
  -p 5005:5005 \
  -p 6006:6006 \
  xrpllabsofficial/xrpld:latest \
  -a

# Check logs
docker logs rippled_standalone

# Access rippled CLI
docker exec rippled_standalone rippled --help
```

**Port Configuration:**

- `5005`: JSON-RPC (HTTP) endpoint
- `6006`: WebSocket endpoint

**Option 2: rippleci Image (CI/CD builds)**

```bash
# Use rippleci image (specific version)
docker run -d \
  --name rippled_standalone \
  -p 5005:5005 \
  -p 6006:6006 \
  rippleci/rippled:2.5.0 \
  standalone
```

#### Manual Ledger Advancement

In standalone mode, ledgers don't advance automatically. You must manually close each ledger after submitting transactions.

```bash
# Method 1: Docker exec
docker exec rippled_standalone rippled ledger_accept

# Method 2: JSON-RPC call
curl -X POST http://localhost:5005 \
  -H "Content-Type: application/json" \
  -d '{
    "method": "ledger_accept",
    "params": []
  }'
```

**Response:**

```json
{
  "result": {
    "ledger_current_index": 2,
    "status": "success"
  }
}
```

#### Configuration File (rippled.cfg)

For advanced configurations, mount a custom `rippled.cfg`:

```ini
# rippled.cfg
[server]
port_rpc_admin_local
port_ws_admin_local

[port_rpc_admin_local]
port = 5005
ip = 0.0.0.0
admin = 127.0.0.1
protocol = http

[port_ws_admin_local]
port = 6006
ip = 0.0.0.0
admin = 127.0.0.1
protocol = ws

[node_size]
small

[ledger_history]
256

[database_path]
/var/lib/rippled/db

[debug_logfile]
/var/log/rippled/debug.log
```

**Mount configuration:**

```bash
docker run -d \
  --name rippled_standalone \
  -p 5005:5005 \
  -p 6006:6006 \
  -v $(pwd)/rippled.cfg:/etc/opt/ripple/rippled.cfg \
  xrpllabsofficial/xrpld:latest \
  -a
```

#### Testing JSON-RPC Endpoint

```bash
# Check server status
curl -X POST http://localhost:5005 \
  -H "Content-Type: application/json" \
  -d '{
    "method": "server_info",
    "params": []
  }'

# Get ledger info
curl -X POST http://localhost:5005 \
  -H "Content-Type: application/json" \
  -d '{
    "method": "ledger",
    "params": [{"ledger_index": "validated"}]
  }'
```

#### Creating Test Accounts

In standalone mode, you can fund accounts without a faucet:

```javascript
// Using xrpl.js library
const xrpl = require('xrpl');

async function createTestAccounts() {
  const client = new xrpl.Client('ws://localhost:6006');
  await client.connect();

  // Generate new wallet
  const wallet = xrpl.Wallet.generate();
  console.log('Address:', wallet.address);
  console.log('Secret:', wallet.seed);

  // Fund account (in standalone mode, genesis account has XRP)
  const payment = {
    TransactionType: 'Payment',
    Account: 'rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh', // Genesis account
    Destination: wallet.address,
    Amount: '1000000000', // 1,000 XRP (in drops)
  };

  const prepared = await client.autofill(payment);
  const signed = client.sign(prepared);
  await client.submitAndWait(signed.tx_blob);

  // Manually advance ledger
  await client.request({ command: 'ledger_accept' });

  console.log('Account funded with 1,000 XRP');
  await client.disconnect();
}

createTestAccounts();
```

---

### 2.2 Payment Channel Configuration

XRPL payment channels are fully supported in standalone mode, allowing comprehensive testing of channel lifecycle operations.

#### Payment Channel Overview

**XRPL Payment Channels:**

- **Unidirectional:** XRP flows from source to destination
- **Claim-based:** Receiver claims XRP with signed authorization
- **Time-locked:** Settlement delay protects against disputes
- **On-ledger:** Channel state stored in XRPL ledger

#### Transaction Types

1. **PaymentChannelCreate:** Opens a new payment channel
2. **PaymentChannelClaim:** Claims XRP or closes channel
3. **PaymentChannelFund:** Adds more XRP to existing channel

#### Example: Create Payment Channel

```json
{
  "TransactionType": "PaymentChannelCreate",
  "Account": "rN7n7otQDd6FczFgLdlqtyMVrn3HMfXEkk",
  "Amount": "100000000",
  "Destination": "rLHzPsX6oXkzU9fXkSuXZvHJiVGgzRx3mR",
  "SettleDelay": 86400,
  "PublicKey": "02F89EAEC7667B30F33D0687BBA86C3FE2A08CCA40A9186C5BDE2DAA6FA97A37D8",
  "Fee": "10"
}
```

**Field Descriptions:**

| Field         | Description                         | Example                              |
| ------------- | ----------------------------------- | ------------------------------------ |
| `Account`     | Channel source (payer)              | `rN7n7otQDd6FczFgLdlqtyMVrn3HMfXEkk` |
| `Amount`      | XRP allocated to channel (in drops) | `100000000` (100 XRP)                |
| `Destination` | Channel destination (payee)         | `rLHzPsX6oXkzU9fXkSuXZvHJiVGgzRx3mR` |
| `SettleDelay` | Seconds before channel can close    | `86400` (24 hours)                   |
| `PublicKey`   | Source's public key for signatures  | `02F89E...`                          |
| `Fee`         | Transaction fee (in drops)          | `10`                                 |

#### Example: Claim from Payment Channel

```json
{
  "TransactionType": "PaymentChannelClaim",
  "Account": "rLHzPsX6oXkzU9fXkSuXZvHJiVGgzRx3mR",
  "Channel": "5DB01B7FFED6B67E6B0414DED11E051D2EE2B7619CE0EAA6286D67A3A4D5BDB3",
  "Amount": "1000000",
  "Balance": "1000000",
  "Signature": "30440220...",
  "PublicKey": "02F89EAEC7667B30F33D0687BBA86C3FE2A08CCA40A9186C5BDE2DAA6FA97A37D8",
  "Fee": "10"
}
```

**Field Descriptions:**

| Field       | Description                           | Example           |
| ----------- | ------------------------------------- | ----------------- |
| `Account`   | Who submits the claim (usually payee) | `rLHzP...`        |
| `Channel`   | Channel ID (64-char hex)              | `5DB01B...`       |
| `Amount`    | Cumulative XRP authorized (in drops)  | `1000000` (1 XRP) |
| `Balance`   | XRP to deliver this transaction       | `1000000`         |
| `Signature` | Source's signature authorizing amount | `30440220...`     |

#### Generating Signatures (Off-Chain)

```javascript
// Using xrpl.js to sign payment channel claims
const xrpl = require('xrpl');

function signClaim(wallet, channelId, amountDrops) {
  const message = channelId + amountDrops.toString(16).toUpperCase().padStart(16, '0');
  const signature = wallet.sign(message);
  return signature;
}

// Example usage
const wallet = xrpl.Wallet.fromSeed('sXXXXXXXXXXXXXXXXXXXXXXXXXX');
const channelId = '5DB01B7FFED6B67E6B0414DED11E051D2EE2B7619CE0EAA6286D67A3A4D5BDB3';
const amount = 1000000; // 1 XRP in drops

const signature = signClaim(wallet, channelId, amount);
console.log('Signature:', signature);
```

#### Payment Channel Lifecycle Testing

**Complete Test Flow:**

1. **Create channel**
   - Source allocates 100 XRP to channel
   - Set 24-hour settlement delay

2. **Off-chain claims**
   - Source signs authorization for 1 XRP
   - Destination receives signed claim
   - Repeat for 2 XRP, 3 XRP, etc. (off-chain)

3. **On-chain settlement**
   - Destination submits claim with latest signature
   - Receives XRP balance

4. **Channel closure**
   - Source or destination closes channel
   - Settlement delay begins
   - Final balance distributed after delay

#### Checking Channel State

```bash
# Query account's payment channels
curl -X POST http://localhost:5005 \
  -H "Content-Type: application/json" \
  -d '{
    "method": "account_channels",
    "params": [{
      "account": "rN7n7otQDd6FczFgLdlqtyMVrn3HMfXEkk",
      "ledger_index": "validated"
    }]
  }'
```

**Response:**

```json
{
  "result": {
    "account": "rN7n7otQDd6FczFgLdlqtyMVrn3HMfXEkk",
    "channels": [
      {
        "account": "rN7n7otQDd6FczFgLdlqtyMVrn3HMfXEkk",
        "amount": "100000000",
        "balance": "1000000",
        "channel_id": "5DB01B7FFED6B67E6B0414DED11E051D2EE2B7619CE0EAA6286D67A3A4D5BDB3",
        "destination_account": "rLHzPsX6oXkzU9fXkSuXZvHJiVGgzRx3mR",
        "settle_delay": 86400,
        "public_key": "02F89EAEC7667B30F33D0687BBA86C3FE2A08CCA40A9186C5BDE2DAA6FA97A37D8"
      }
    ]
  }
}
```

---

## 3. Docker Compose Integration

Integrate both Base L2 (Anvil) and XRPL (rippled) into a unified Docker Compose setup for streamlined development.

### Complete docker-compose.yml

```yaml
version: '3.8'

services:
  # Base L2 local node (Anvil)
  anvil:
    image: ghcr.io/foundry-rs/foundry:latest
    container_name: anvil_base_local
    command: >
      anvil
      --host 0.0.0.0
      --port 8545
      --fork-url ${BASE_SEPOLIA_RPC_URL}
      --fork-block-number ${FORK_BLOCK_NUMBER:-20702367}
      --chain-id 84532
      --optimism
    ports:
      - '8545:8545'
    networks:
      - blockchain_network
    environment:
      - BASE_SEPOLIA_RPC_URL=${BASE_SEPOLIA_RPC_URL}
    healthcheck:
      test:
        [
          'CMD',
          'sh',
          '-c',
          'curl -f http://localhost:8545 -X POST -H ''Content-Type: application/json'' --data ''{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}'' || exit 1',
        ]
      interval: 10s
      timeout: 5s
      retries: 5
      start_period: 10s

  # XRP Ledger standalone node (rippled)
  rippled:
    image: xrpllabsofficial/xrpld:latest
    container_name: rippled_standalone
    command: ['-a']
    ports:
      - '5005:5005' # JSON-RPC
      - '6006:6006' # WebSocket
    networks:
      - blockchain_network
    healthcheck:
      test:
        [
          'CMD',
          'sh',
          '-c',
          'curl -f http://localhost:5005 -X POST -H ''Content-Type: application/json'' --data ''{"method":"server_info","params":[]}'' || exit 1',
        ]
      interval: 10s
      timeout: 5s
      retries: 5
      start_period: 15s
    volumes:
      - rippled_data:/var/lib/rippled

  # Optional: Helper service for automated ledger advancement
  rippled_ledger_advancer:
    image: curlimages/curl:latest
    container_name: rippled_ledger_advancer
    networks:
      - blockchain_network
    depends_on:
      rippled:
        condition: service_healthy
    command: >
      sh -c "
      while true; do
        sleep 5;
        curl -X POST http://rippled:5005 -H 'Content-Type: application/json' --data '{\"method\":\"ledger_accept\",\"params\":[]}';
      done
      "
    restart: unless-stopped

networks:
  blockchain_network:
    driver: bridge

volumes:
  rippled_data:
```

### Environment Variables (.env)

```bash
# .env file
BASE_SEPOLIA_RPC_URL=https://sepolia.base.org
FORK_BLOCK_NUMBER=20702367
```

### Starting Services

```bash
# Start all blockchain nodes
docker-compose up -d

# Check service status
docker-compose ps

# View logs
docker-compose logs -f anvil
docker-compose logs -f rippled

# Stop all services
docker-compose down

# Stop and remove volumes (reset state)
docker-compose down -v
```

### Health Checks

**Anvil health check:**

```bash
curl -X POST http://localhost:8545 \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "method": "eth_blockNumber",
    "params": [],
    "id": 1
  }'
```

**rippled health check:**

```bash
curl -X POST http://localhost:5005 \
  -H "Content-Type: application/json" \
  -d '{
    "method": "server_info",
    "params": []
  }'
```

### Service Dependencies

```yaml
# Example: Connector service depending on blockchain nodes
connector:
  image: agent-runtime/connector:latest
  depends_on:
    anvil:
      condition: service_healthy
    rippled:
      condition: service_healthy
  environment:
    - BASE_RPC_URL=http://anvil:8545
    - XRPL_RPC_URL=http://rippled:5005
```

---

## 4. Testing Payment Channels

### 4.1 EVM Payment Channel Testing

#### Smart Contract Setup

**Example: Simple EVM Payment Channel Contract**

```solidity
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import "@openzeppelin/contracts/security/ReentrancyGuard.sol";
import "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";

contract SimplePaymentChannel is ReentrancyGuard {
    using SafeERC20 for IERC20;
    using ECDSA for bytes32;

    struct Channel {
        address sender;
        address receiver;
        uint256 deposit;
        uint256 nonce;
        uint256 closedAt;
        bool closed;
    }

    mapping(bytes32 => Channel) public channels;

    event ChannelOpened(bytes32 indexed channelId, address sender, address receiver, uint256 deposit);
    event ChannelClosed(bytes32 indexed channelId, uint256 amount);

    function openChannel(address receiver, uint256 amount) external payable nonReentrant {
        require(msg.value == amount, "Deposit mismatch");

        bytes32 channelId = keccak256(abi.encodePacked(msg.sender, receiver, block.timestamp));

        channels[channelId] = Channel({
            sender: msg.sender,
            receiver: receiver,
            deposit: amount,
            nonce: 0,
            closedAt: 0,
            closed: false
        });

        emit ChannelOpened(channelId, msg.sender, receiver, amount);
    }

    function closeChannel(bytes32 channelId, uint256 amount, uint256 nonce, bytes memory signature) external nonReentrant {
        Channel storage channel = channels[channelId];
        require(!channel.closed, "Channel already closed");
        require(msg.sender == channel.receiver, "Only receiver can close");

        // Verify signature
        bytes32 message = keccak256(abi.encodePacked(channelId, amount, nonce));
        bytes32 ethSignedMessage = message.toEthSignedMessageHash();
        address signer = ethSignedMessage.recover(signature);
        require(signer == channel.sender, "Invalid signature");
        require(nonce > channel.nonce, "Nonce must increase");
        require(amount <= channel.deposit, "Amount exceeds deposit");

        channel.closed = true;
        channel.closedAt = block.timestamp;

        // Transfer funds
        payable(channel.receiver).transfer(amount);
        if (channel.deposit > amount) {
            payable(channel.sender).transfer(channel.deposit - amount);
        }

        emit ChannelClosed(channelId, amount);
    }
}
```

#### Testing with Foundry

```solidity
// test/SimplePaymentChannel.t.sol
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/SimplePaymentChannel.sol";

contract SimplePaymentChannelTest is Test {
    SimplePaymentChannel channel;
    address sender = address(0x1);
    address receiver = address(0x2);

    function setUp() public {
        channel = new SimplePaymentChannel();
        vm.deal(sender, 10 ether);
    }

    function testOpenChannel() public {
        vm.prank(sender);
        channel.openChannel{value: 1 ether}(receiver, 1 ether);
        // Assertions...
    }

    function testCloseChannel() public {
        // Open channel
        vm.prank(sender);
        bytes32 channelId = keccak256(abi.encodePacked(sender, receiver, block.timestamp));
        channel.openChannel{value: 1 ether}(receiver, 1 ether);

        // Generate signature (off-chain simulation)
        uint256 amount = 0.5 ether;
        uint256 nonce = 1;
        bytes32 message = keccak256(abi.encodePacked(channelId, amount, nonce));
        // ... sign message ...

        // Close channel
        vm.prank(receiver);
        channel.closeChannel(channelId, amount, nonce, signature);
        // Assertions...
    }
}
```

**Run tests:**

```bash
forge test -vvv
```

#### Deployment to Local Anvil

```bash
# Start Anvil
anvil

# Deploy contract
forge create src/SimplePaymentChannel.sol:SimplePaymentChannel \
  --rpc-url http://localhost:8545 \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
```

#### Off-Chain Signature Generation (TypeScript)

```typescript
import { ethers } from 'ethers';

async function signPaymentChannelClaim(
  signer: ethers.Wallet,
  channelId: string,
  amount: bigint,
  nonce: number
): Promise<string> {
  const message = ethers.solidityPackedKeccak256(
    ['bytes32', 'uint256', 'uint256'],
    [channelId, amount, nonce]
  );

  const signature = await signer.signMessage(ethers.getBytes(message));
  return signature;
}

// Example usage
const provider = new ethers.JsonRpcProvider('http://localhost:8545');
const wallet = new ethers.Wallet(
  '0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80',
  provider
);

const channelId = '0x5db01b...'; // From contract
const amount = ethers.parseEther('0.5');
const nonce = 1;

const signature = await signPaymentChannelClaim(wallet, channelId, amount, nonce);
console.log('Signature:', signature);
```

#### Complete Testing Workflow

1. **Deploy contract to Anvil**

   ```bash
   forge create SimplePaymentChannel --rpc-url http://localhost:8545 --private-key <KEY>
   ```

2. **Open payment channel**

   ```bash
   cast send <CONTRACT> "openChannel(address,uint256)" <RECEIVER> 1000000000000000000 \
     --rpc-url http://localhost:8545 --private-key <KEY> --value 1ether
   ```

3. **Sign off-chain payment**

   ```typescript
   const signature = await signPaymentChannelClaim(wallet, channelId, amount, nonce);
   ```

4. **Close channel with signature**
   ```bash
   cast send <CONTRACT> "closeChannel(bytes32,uint256,uint256,bytes)" \
     <CHANNEL_ID> <AMOUNT> <NONCE> <SIGNATURE> \
     --rpc-url http://localhost:8545 --private-key <RECEIVER_KEY>
   ```

---

### 4.2 XRPL Payment Channel Testing

#### Complete Payment Channel Test Script (JavaScript)

```javascript
const xrpl = require('xrpl');

async function testXRPLPaymentChannel() {
  // Connect to local rippled
  const client = new xrpl.Client('ws://localhost:6006');
  await client.connect();

  // Create sender and receiver wallets
  const sender = xrpl.Wallet.generate();
  const receiver = xrpl.Wallet.generate();

  console.log('Sender:', sender.address);
  console.log('Receiver:', receiver.address);

  // Fund accounts from genesis (standalone mode only)
  await fundAccount(client, sender.address, '1000000000'); // 1,000 XRP
  await fundAccount(client, receiver.address, '100000000'); // 100 XRP

  // 1. Create payment channel
  const channelTx = {
    TransactionType: 'PaymentChannelCreate',
    Account: sender.address,
    Amount: '100000000', // 100 XRP
    Destination: receiver.address,
    SettleDelay: 86400, // 24 hours
    PublicKey: sender.publicKey,
  };

  const preparedChannel = await client.autofill(channelTx);
  const signedChannel = sender.sign(preparedChannel);
  const resultChannel = await client.submitAndWait(signedChannel.tx_blob);

  await client.request({ command: 'ledger_accept' });

  const channelId = resultChannel.result.meta.AffectedNodes.find(
    (node) => node.CreatedNode?.LedgerEntryType === 'PayChannel'
  )?.CreatedNode.LedgerIndex;

  console.log('Channel Created:', channelId);

  // 2. Sign off-chain claim
  const claimAmount = '1000000'; // 1 XRP
  const signature = signClaim(sender, channelId, claimAmount);
  console.log('Claim Signature:', signature);

  // 3. Receiver claims XRP
  const claimTx = {
    TransactionType: 'PaymentChannelClaim',
    Account: receiver.address,
    Channel: channelId,
    Amount: claimAmount,
    Balance: claimAmount,
    Signature: signature.toUpperCase(),
    PublicKey: sender.publicKey,
  };

  const preparedClaim = await client.autofill(claimTx);
  const signedClaim = receiver.sign(preparedClaim);
  const resultClaim = await client.submitAndWait(signedClaim.tx_blob);

  await client.request({ command: 'ledger_accept' });

  console.log('Claim Result:', resultClaim.result.meta.TransactionResult);

  // 4. Check channel state
  const channels = await client.request({
    command: 'account_channels',
    account: sender.address,
  });

  console.log('Channel State:', JSON.stringify(channels.result.channels, null, 2));

  await client.disconnect();
}

// Helper: Fund account in standalone mode
async function fundAccount(client, address, amount) {
  const payment = {
    TransactionType: 'Payment',
    Account: 'rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh', // Genesis account
    Destination: address,
    Amount: amount,
  };

  const prepared = await client.autofill(payment);
  const result = await client.submitAndWait(prepared);
  await client.request({ command: 'ledger_accept' });

  console.log(`Funded ${address} with ${xrpl.dropsToXrp(amount)} XRP`);
}

// Helper: Sign payment channel claim
function signClaim(wallet, channelId, amountDrops) {
  const claim = channelId + Number(amountDrops).toString(16).toUpperCase().padStart(16, '0');
  return wallet.sign(claim).signature;
}

// Run test
testXRPLPaymentChannel().catch(console.error);
```

#### Running the Test

```bash
# Install xrpl.js
npm install xrpl

# Ensure rippled is running
docker ps | grep rippled_standalone

# Run test script
node test-payment-channel.js
```

#### Expected Output

```
Sender: rN7n7otQDd6FczFgLdlqtyMVrn3HMfXEkk
Receiver: rLHzPsX6oXkzU9fXkSuXZvHJiVGgzRx3mR
Funded rN7n7otQDd6FczFgLdlqtyMVrn3HMfXEkk with 1000 XRP
Funded rLHzPsX6oXkzU9fXkSuXZvHJiVGgzRx3mR with 100 XRP
Channel Created: 5DB01B7FFED6B67E6B0414DED11E051D2EE2B7619CE0EAA6286D67A3A4D5BDB3
Claim Signature: 304402201F3D...
Claim Result: tesSUCCESS
Channel State: [
  {
    "account": "rN7n7otQDd6FczFgLdlqtyMVrn3HMfXEkk",
    "amount": "100000000",
    "balance": "1000000",
    "channel_id": "5DB01B7FFED6B67E6B0414DED11E051D2EE2B7619CE0EAA6286D67A3A4D5BDB3",
    "destination_account": "rLHzPsX6oXkzU9fXkSuXZvHJiVGgzRx3mR",
    "settle_delay": 86400
  }
]
```

---

## 5. Development Workflow Integration

### Account and Wallet Setup

#### Base L2 (Anvil) - Pre-funded Accounts

Anvil provides 10 pre-funded test accounts with 10,000 ETH each:

```
Account #0: 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266
Private Key: 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80

Account #1: 0x70997970C51812dc3A010C7d01b50e0d17dc79C8
Private Key: 0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d

... (8 more accounts)
```

**WARNING:** Never use these on mainnet - they are public knowledge.

#### XRP Ledger - Testnet Faucet (Programmatic)

For XRP Testnet (not standalone), use the automated faucet:

```bash
# Testnet faucet
curl -X POST https://faucet.altnet.rippletest.net/accounts

# Devnet faucet
curl -X POST https://faucet.devnet.rippletest.net/accounts
```

**Response:**

```json
{
  "account": {
    "address": "rPT1Sjq2YGrBMTttX4GZHjKu9dyfzbpAYe",
    "secret": "s████████████████████████████"
  },
  "balance": 100
}
```

**Funding limits (2025):**

- Default: 100 XRP per request
- Maximum: 1,000 XRP per request

#### Private Key Management

**For local development:**

```bash
# .env file (never commit to git)
PRIVATE_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
BASE_RPC_URL=http://localhost:8545
XRPL_RPC_URL=http://localhost:5005
```

**Best practices:**

- Use separate keys for each network (local, testnet, mainnet)
- Never hardcode private keys in source code
- Use environment variables or secret management (Vault, AWS Secrets Manager)
- Rotate keys regularly for production

---

### Reset and Restart Procedures

#### Reset Anvil State

**Option 1: Restart Anvil (ephemeral mode)**

```bash
# Stop Anvil (Ctrl+C)
# Restart Anvil - state is lost
anvil --fork-url https://sepolia.base.org
```

**Option 2: Use state snapshots**

```bash
# Save state before resetting
cast rpc evm_snapshot --rpc-url http://localhost:8545

# Returns snapshot ID: 0x1

# Revert to snapshot
cast rpc evm_revert 0x1 --rpc-url http://localhost:8545
```

#### Reset rippled State

**Option 1: Restart container**

```bash
docker restart rippled_standalone
```

**Option 2: Recreate container (clean state)**

```bash
docker rm -f rippled_standalone
docker run -d --name rippled_standalone -p 5005:5005 -p 6006:6006 xrpllabsofficial/xrpld:latest -a
```

---

### Monitoring and Debugging

#### Anvil Debugging

**Enable verbose logging:**

```bash
anvil --fork-url https://sepolia.base.org -vvv
```

**Monitor transactions:**

```bash
# Watch latest block
watch -n 1 'cast block-number --rpc-url http://localhost:8545'

# View transaction receipt
cast receipt <TX_HASH> --rpc-url http://localhost:8545
```

#### rippled Debugging

**View logs:**

```bash
docker logs -f rippled_standalone
```

**Check transaction status:**

```bash
curl -X POST http://localhost:5005 \
  -H "Content-Type: application/json" \
  -d '{
    "method": "tx",
    "params": [{
      "transaction": "<TX_HASH>"
    }]
  }'
```

**Monitor ledger advancement:**

```bash
watch -n 1 'curl -s -X POST http://localhost:5005 -H "Content-Type: application/json" -d "{\"method\":\"ledger\",\"params\":[]}" | jq .result.ledger_index'
```

---

## 6. Performance and Resource Requirements

### Resource Comparison Table

| Component                | CPU       | RAM    | Disk        | Startup Time     | Block Time                      |
| ------------------------ | --------- | ------ | ----------- | ---------------- | ------------------------------- |
| **Anvil (local)**        | 1-2 cores | 1-2 GB | Minimal     | < 5 seconds      | Instant                         |
| **Anvil (forked)**       | 2-4 cores | 2-4 GB | Minimal     | 10-30 seconds    | Instant                         |
| **Hardhat**              | 2-4 cores | 2-4 GB | Minimal     | 10-30 seconds    | Configurable (default: instant) |
| **Base Full Node**       | 8+ cores  | 16+ GB | 2+ TB NVMe  | 1-3 days (sync)  | 2 seconds                       |
| **rippled (standalone)** | 2-4 cores | 4-8 GB | 10 GB       | < 10 seconds     | Manual (instant)                |
| **rippled (production)** | 8+ cores  | 16+ GB | 50+ GB NVMe | 2-6 hours (sync) | 3-5 seconds                     |

### Performance Benchmarks

**Anvil (Foundry):**

- **Transaction throughput:** 1,200+ tests/second
- **Contract deployment:** < 100ms
- **State queries:** < 10ms
- **Fork initialization:** 10-30 seconds (depends on RPC latency)

**Hardhat:**

- **Transaction throughput:** 450 tests/second
- **Contract deployment:** < 200ms
- **State queries:** < 20ms

**rippled (standalone):**

- **Transaction submission:** < 50ms
- **Ledger advancement:** < 100ms (manual)
- **State queries:** < 10ms

### Typical Block Time and Confirmation Latency

| Network                | Block Time  | Finality    | Confirmation Time |
| ---------------------- | ----------- | ----------- | ----------------- |
| Anvil (local)          | Instant     | Instant     | < 1ms             |
| Base Sepolia (testnet) | ~2 seconds  | ~2 seconds  | 2-4 seconds       |
| Base Mainnet           | ~2 seconds  | ~2 seconds  | 2-4 seconds       |
| XRPL (standalone)      | Manual      | Instant     | < 100ms           |
| XRPL (testnet)         | 3-5 seconds | 3-5 seconds | 3-5 seconds       |

---

## 7. Troubleshooting and Debugging

### Common Issues and Solutions

#### Anvil Issues

**Problem: "error sending request for url"**

- **Cause:** RPC endpoint unreachable or rate-limited
- **Solution:**

  ```bash
  # Use alternative RPC endpoint
  anvil --fork-url https://base.gateway.tenderly.co

  # Or use paid provider (Alchemy, QuickNode)
  anvil --fork-url https://base-sepolia.g.alchemy.com/v2/YOUR_API_KEY
  ```

**Problem: "fork block mismatch"**

- **Cause:** Forked state diverged from upstream
- **Solution:**
  ```bash
  # Pin to specific block
  anvil --fork-url https://sepolia.base.org --fork-block-number 20702367
  ```

**Problem: "out of gas"**

- **Cause:** Gas limit too low for forked state
- **Solution:**
  ```bash
  # Increase gas limit
  anvil --gas-limit 30000000
  ```

#### rippled Issues

**Problem: "Could not connect to server"**

- **Cause:** rippled container not running or port conflict
- **Solution:**

  ```bash
  # Check if container is running
  docker ps | grep rippled

  # Check logs
  docker logs rippled_standalone

  # Restart container
  docker restart rippled_standalone
  ```

**Problem: "Transaction failed: tefPAST_SEQ"**

- **Cause:** Sequence number mismatch (ledger not advanced)
- **Solution:**
  ```bash
  # Advance ledger after each transaction
  curl -X POST http://localhost:5005 \
    -H "Content-Type: application/json" \
    -d '{"method":"ledger_accept","params":[]}'
  ```

**Problem: "tecNO_DST_INSUF_XRP"**

- **Cause:** Destination account doesn't exist or has insufficient XRP reserve
- **Solution:**
  ```bash
  # Fund account with minimum reserve (10 XRP + transaction fee)
  # In standalone mode, use genesis account
  ```

#### Docker Compose Issues

**Problem: "service 'anvil' failed to build"**

- **Cause:** Foundry image not available
- **Solution:**
  ```bash
  # Pull latest Foundry image
  docker pull ghcr.io/foundry-rs/foundry:latest
  ```

**Problem: "unhealthy" service status**

- **Cause:** Health check failing
- **Solution:**

  ```bash
  # Check service logs
  docker-compose logs anvil
  docker-compose logs rippled

  # Test health check manually
  curl -X POST http://localhost:8545 -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}'
  ```

---

### Debugging Tools

#### For EVM (Anvil/Hardhat)

**Foundry tools:**

```bash
# Trace transaction execution
cast run <TX_HASH> --rpc-url http://localhost:8545 --trace

# Decode transaction input
cast 4byte-decode <INPUT_DATA>

# Get storage slot
cast storage <CONTRACT> <SLOT> --rpc-url http://localhost:8545
```

**Hardhat console:**

```bash
npx hardhat console --network localhost
```

#### For XRPL (rippled)

**xrpl.js debugging:**

```javascript
const client = new xrpl.Client('ws://localhost:6006', { trace: true });
```

**Manual transaction inspection:**

```bash
curl -X POST http://localhost:5005 \
  -H "Content-Type: application/json" \
  -d '{
    "method": "tx",
    "params": [{
      "transaction": "<TX_HASH>",
      "binary": false
    }]
  }' | jq
```

---

## 8. References and Additional Resources

### Official Documentation

**Base L2:**

- Base Docs: https://docs.base.org/
- Base GitHub: https://github.com/base/node
- Base Sepolia Faucet: https://www.alchemy.com/faucets/base-sepolia
- Basescan Explorer: https://sepolia.basescan.org/

**Foundry (Anvil):**

- Foundry Book: https://book.getfoundry.sh/
- Anvil Reference: https://book.getfoundry.sh/reference/anvil/
- Foundry GitHub: https://github.com/foundry-rs/foundry

**Hardhat:**

- Hardhat Docs: https://hardhat.org/docs
- Hardhat Network: https://hardhat.org/hardhat-network/docs
- Base Integration: https://docs.base.org/learn/hardhat/hardhat-setup-overview

**XRP Ledger:**

- XRPL Docs: https://xrpl.org/docs
- Payment Channels: https://xrpl.org/docs/concepts/payment-types/payment-channels
- rippled Setup: https://xrpl.org/docs/infrastructure/installation/system-requirements
- xrpl.js Library: https://js.xrpl.org/

**Docker:**

- Docker Compose: https://docs.docker.com/compose/
- Health Checks: https://docs.docker.com/engine/reference/builder/#healthcheck

### GitHub Examples

**Payment Channel Implementations:**

- Raiden Network (EVM): https://github.com/raiden-network/raiden-contracts
- Connext (EVM): https://github.com/connext/contracts
- ERC-1630 HTLC: https://github.com/ethereum/EIPs/pull/1630
- XRPL Payment Channels: https://github.com/XRPLF/xrpl.js/wiki/Using-the-Testnet-and-Devnet-faucets-programmatically

**Local Node Setups:**

- Base Node Docker: https://github.com/base/node
- rippled Docker: https://github.com/WietseWind/docker-rippled
- OP Stack Devnet: https://github.com/op-rs/op-up

### Community Resources

**Forums and Support:**

- Base Discord: https://discord.gg/buildonbase
- Foundry Telegram: https://t.me/foundry_support
- XRPL Developer Discord: https://discord.gg/xrpl
- Ethereum Stack Exchange: https://ethereum.stackexchange.com/

**Tutorials:**

- Base Smart Contract Development: https://docs.base.org/cookbook/smart-contract-development/
- Foundry Tutorial: https://updraft.cyfrin.io/courses/foundry
- XRPL Payment Channels Tutorial: https://xrpl.org/docs/tutorials/how-tos/use-specialized-payment-types/use-payment-channels

---

## Appendix A: Complete Configuration Examples

### Environment Variables Template

```bash
# .env.example - Copy to .env and fill in values

# Base L2 Configuration
BASE_SEPOLIA_RPC_URL=https://sepolia.base.org
BASE_MAINNET_RPC_URL=https://mainnet.base.org
BASE_FORK_BLOCK_NUMBER=20702367

# Alternative RPC Providers (optional)
ALCHEMY_API_KEY=your_alchemy_api_key
QUICKNODE_API_KEY=your_quicknode_api_key

# Development Private Keys (LOCAL TESTING ONLY)
ANVIL_PRIVATE_KEY_0=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
ANVIL_PRIVATE_KEY_1=0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d

# Production Private Keys (use secret manager)
PRODUCTION_PRIVATE_KEY=  # DO NOT COMMIT

# XRPL Configuration
XRPL_TESTNET_RPC=https://api.altnet.rippletest.net:51234
XRPL_MAINNET_RPC=wss://xrplcluster.com/

# Etherscan API (for contract verification)
ETHERSCAN_API_KEY=your_etherscan_api_key
```

### Complete Foundry Project Structure

```
packages/contracts/
├── foundry.toml
├── script/
│   ├── Deploy.s.sol
│   └── DeployLocal.s.sol
├── src/
│   ├── TokenNetwork.sol
│   ├── TokenNetworkRegistry.sol
│   └── interfaces/
│       └── IPaymentChannel.sol
├── test/
│   ├── TokenNetwork.t.sol
│   ├── TokenNetworkRegistry.t.sol
│   └── integration/
│       └── FullFlow.t.sol
├── lib/
│   └── openzeppelin-contracts/
└── .env
```

---

## Appendix B: Testing Checklists

### Base L2 (EVM) Payment Channel Testing Checklist

- [ ] Anvil running locally (http://localhost:8545)
- [ ] Smart contract deployed to local Anvil
- [ ] Contract verified on Base Sepolia testnet
- [ ] Channel opened with initial deposit
- [ ] Off-chain signature generation working
- [ ] Channel closure with valid signature
- [ ] Challenge period tested (dispute resolution)
- [ ] Final settlement distributes funds correctly
- [ ] Multiple channels can coexist
- [ ] Reentrancy protection verified
- [ ] Gas costs measured and optimized

### XRPL Payment Channel Testing Checklist

- [ ] rippled standalone running (http://localhost:5005)
- [ ] Test accounts created and funded
- [ ] PaymentChannelCreate transaction successful
- [ ] Channel ID retrieved from transaction metadata
- [ ] Off-chain claim signature generated
- [ ] PaymentChannelClaim transaction successful
- [ ] Channel balance updated correctly
- [ ] Settlement delay enforced
- [ ] Channel closure tested
- [ ] Multiple claims tested (increasing amounts)
- [ ] Manual ledger advancement working

---

## Conclusion

This guide provides everything needed to set up local Base L2 and XRP Ledger nodes for payment channel development in the M2M project. The recommended approach combines:

1. **Anvil** for fast, cost-free Base L2 EVM development
2. **rippled standalone mode** for comprehensive XRPL payment channel testing
3. **Docker Compose** for unified orchestration and easy reproducibility

With these tools, developers can iterate rapidly on payment channel implementations, test edge cases locally, and deploy to testnets/mainnets with confidence.

**Next Steps:**

1. Follow Story 7.1 in Epic 7 to set up the Foundry development environment
2. Deploy payment channel smart contracts to local Anvil
3. Integrate with Epic 6's settlement monitoring for automated on-chain settlement
4. Test end-to-end payment channel lifecycle with both EVM and XRPL implementations

**Estimated Total Setup Time:** 3-5 hours for complete dual-chain local development environment.
