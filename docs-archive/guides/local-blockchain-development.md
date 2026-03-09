# Local Blockchain Development Guide

## Introduction

This guide explains how to set up and use a local blockchain node for M2M project development. A local blockchain node enables rapid development and testing of payment channel smart contracts without relying on public testnets or mainnets.

**Purpose:**

- **EVM Development**: Local blockchain infrastructure for Base L2 (EVM) smart contract development
- **Anvil (Base L2)**: Local Ethereum node forking Base Sepolia testnet

**Benefits of Local Blockchain Development:**

- ⚡ **Instant blocks**: No waiting for block confirmation times (instant mining)
- 💰 **Zero gas costs**: Deploy and test contracts without spending real ETH
- 📍 **State pinning**: Consistent blockchain state across developer machines
- 🚫 **No rate limits**: Unlimited RPC requests without API key restrictions
- 🔌 **Offline development**: Work without internet connection after initial fork download
- 🎯 **Deterministic testing**: Same pre-funded accounts and state for reproducible tests

## Quick Start (5 minutes)

### Prerequisites

Before starting, ensure you have the following installed:

- **Docker Desktop**: 20.10+ ([Download](https://www.docker.com/products/docker-desktop))
- **Node.js**: 20.11.0 LTS ([Download](https://nodejs.org/))
- **npm**: 10.x (included with Node.js)
- **Git**: 2.x ([Download](https://git-scm.com/))
- **curl**: Pre-installed on macOS/Linux, or use Git Bash on Windows

### Setup Steps

**Step 1: Clone the M2M Repository**

```bash
git clone <repository-url>
cd m2m
```

**Step 2: Configure Environment Variables**

```bash
cp .env.dev.example .env.dev
```

Edit `.env.dev` if you need to customize the RPC endpoint or fork block number.

**Step 3: Start Local Blockchain Nodes**

```bash
# Start all development services
docker-compose -f docker-compose-dev.yml up -d

# Or start only Anvil (Base L2 node)
docker-compose -f docker-compose-dev.yml up -d anvil
```

**Step 4: Verify Anvil is Running**

```bash
curl -X POST http://localhost:8545 \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}'
```

**Expected output:**

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": "0x13c377f"
}
```

**Step 5: Start Developing!**

Your local blockchain node is ready:

**Anvil (Base L2):**

- **Host machine**: `http://localhost:8545`
- **Docker containers**: `http://anvil:8545`

## Anvil (Base L2) Setup

### What is Anvil?

Anvil is Foundry's local Ethereum node, optimized for testing and development. It provides:

- **Fast blockchain simulation**: Instant block mining on transaction submission
- **State forking**: Download and fork existing blockchain state from any network
- **OP Stack support**: Full Optimism/Base L2 compatibility with `--optimism` flag
- **Pre-funded accounts**: 10 deterministic test accounts with 10000 ETH each

**Purpose in M2M Project:**

Anvil provides a local Base L2 fork for EVM Payment Channels development. Developers can deploy and test payment channel smart contracts locally without testnet dependencies or rate limits.

### Anvil Configuration

Anvil is configured in `docker-compose-dev.yml` with the following settings:

| Configuration           | Value                      | Purpose                                        |
| ----------------------- | -------------------------- | ---------------------------------------------- |
| **Fork URL**            | `https://sepolia.base.org` | Download Base Sepolia testnet state            |
| **Fork Block**          | `20702367` (configurable)  | Pin to specific block for consistent state     |
| **Chain ID**            | `84532`                    | Base Sepolia chain ID (matches public testnet) |
| **OP Stack Flag**       | `--optimism`               | Enable OP Stack opcodes and gas calculations   |
| **Port**                | `8545`                     | Standard Ethereum JSON-RPC port                |
| **Pre-funded Accounts** | 10 accounts                | Each account has 10000 ETH for testing         |

**Environment Variables (configured in `.env.dev`):**

```bash
# Base Sepolia RPC endpoint for forking blockchain state
BASE_SEPOLIA_RPC_URL=https://sepolia.base.org

# Pinned block number for consistent state
FORK_BLOCK_NUMBER=20702367
```

### Pre-funded Test Accounts

Anvil automatically generates 10 pre-funded accounts with deterministic addresses and private keys. These accounts are **identical across all Anvil instances**, enabling reproducible testing.

**Account #0** (Primary test account):

- **Address**: `0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266`
- **Private Key**: `0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80`
- **Initial Balance**: 10000 ETH

**Account #1**:

- **Address**: `0x70997970C51812dc3A010C7d01b50e0d17dc79C8`
- **Private Key**: `0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d`
- **Initial Balance**: 10000 ETH

**Account #2**:

- **Address**: `0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC`
- **Private Key**: `0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a`
- **Initial Balance**: 10000 ETH

_7 additional accounts available (see Anvil logs for full list)_

### Connecting to Anvil

#### RPC Endpoints

- **From host machine**: `http://localhost:8545`
- **From Docker containers**: `http://anvil:8545`

#### Configure MetaMask

To connect MetaMask to your local Anvil instance:

1. Open MetaMask and click the network dropdown
2. Select "Add Network" → "Add a network manually"
3. Enter the following details:
   - **Network Name**: Anvil Local (Base Sepolia Fork)
   - **RPC URL**: `http://localhost:8545`
   - **Chain ID**: `84532`
   - **Currency Symbol**: ETH
4. Click "Save"
5. Import Account #0 using the private key above for testing

#### Configure Foundry (forge/cast)

If you have Foundry installed locally, you can interact with Anvil using `forge` and `cast`:

**Deploy a smart contract:**

```bash
forge create --rpc-url http://localhost:8545 \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
  src/MyContract.sol:MyContract
```

**Check account balance:**

```bash
cast balance 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266 \
  --rpc-url http://localhost:8545
```

**Send a transaction:**

```bash
cast send 0x70997970C51812dc3A010C7d01b50e0d17dc79C8 \
  --value 1ether \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
  --rpc-url http://localhost:8545
```

## Testing Anvil

### Test 1: Get Current Block Number

**Command:**

```bash
curl -X POST http://localhost:8545 \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}'
```

**Expected Result:**

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": "0x13c377f"
}
```

The `result` field contains the current block number in hex format (e.g., `0x13c377f` = 20702367 in decimal).

### Test 2: Get Pre-funded Account Balance

**Command (using cast):**

```bash
cast balance 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266 --rpc-url http://localhost:8545
```

**Expected Result:**

```
10000000000000000000000
```

This is 10000 ETH in wei (10000 \* 10^18).

**Alternative (using curl):**

```bash
curl -X POST http://localhost:8545 \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc":"2.0",
    "method":"eth_getBalance",
    "params":["0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266", "latest"],
    "id":1
  }'
```

### Test 3: Send Test Transaction

**Command (using cast):**

```bash
cast send 0x70997970C51812dc3A010C7d01b50e0d17dc79C8 \
  --value 1ether \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
  --rpc-url http://localhost:8545
```

**Expected Result:**

```
blockHash               0x1234567890abcdef...
blockNumber             20702368
transactionHash         0xabcdef1234567890...
transactionIndex        0
status                  1 (success)
```

Transaction should confirm **instantly** (Anvil auto-mines on transaction submission).

## Development Workflows

### Overview of Development Lifecycle

The M2M project uses an EVM-based blockchain development workflow for smart contracts on Base L2. Understanding the complete development lifecycle helps you ship high-quality code through systematic progression from local testing to production deployment.

**Standard Development Lifecycle:**

```
Setup → Develop → Test Locally → Deploy Testnet → Audit → Deploy Mainnet
```

**Phase Breakdown:**

| Phase              | Purpose                                          | Tools Used                   | Time Estimate             |
| ------------------ | ------------------------------------------------ | ---------------------------- | ------------------------- |
| **Setup**          | Configure local blockchain node and dependencies | Docker, Foundry              | 5-10 minutes              |
| **Develop**        | Write smart contracts and payment channel logic  | Solidity, TypeScript, VSCode | Hours to days             |
| **Test Locally**   | Run tests against Anvil                          | forge test, Jest, curl       | Seconds to minutes        |
| **Deploy Testnet** | Deploy to Base Sepolia                           | forge script                 | 2-5 minutes               |
| **Audit**          | Security review and gas optimization             | Slither, manual review       | Days to weeks             |
| **Deploy Mainnet** | Production deployment to Base mainnet            | forge script                 | 5-10 minutes + monitoring |

**Key Principles:**

- **Always test locally first**: Anvil provides instant feedback with zero costs
- **Never skip testnet**: Production-like testing catches network-specific issues
- **Separate private keys**: Use different keys for development, testnet, and mainnet (NEVER reuse)
- **Monitor mainnet deployments**: Watch for 24 hours before full rollout

### Smart Contract Development Workflow

This workflow guides you through developing, testing, and deploying EVM smart contracts for payment channels on Base L2.

#### Step 1: Write Solidity Contract

Create your smart contract in `packages/contracts/src/`. For this example, we'll use a payment channel contract skeleton.

**File: `packages/contracts/src/PaymentChannel.sol`**

```solidity
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @title PaymentChannel
/// @notice Simple payment channel for ILP settlement
contract PaymentChannel {
    address public sender;
    address public recipient;
    uint256 public expiresAt;

    /// @notice Emitted when a payment channel is created
    event ChannelCreated(address indexed sender, address indexed recipient, uint256 expiresAt);

    /// @notice Create a new payment channel
    /// @param _recipient Address receiving payments
    /// @param _duration Channel duration in seconds
    constructor(address _recipient, uint256 _duration) payable {
        require(msg.value > 0, "Must fund channel with ETH");
        require(_recipient != address(0), "Invalid recipient");

        sender = msg.sender;
        recipient = _recipient;
        expiresAt = block.timestamp + _duration;

        emit ChannelCreated(sender, recipient, expiresAt);
    }

    /// @notice Get channel balance
    function getBalance() public view returns (uint256) {
        return address(this).balance;
    }
}
```

**Contract Structure Explanation:**

- **pragma**: Specifies Solidity compiler version (0.8.20 for this project)
- **State variables**: Persistent storage (sender, recipient, expiresAt)
- **Events**: Emit logs for off-chain indexing
- **Constructor**: Initialize contract state on deployment
- **View functions**: Read-only functions that don't modify state

**Reference**: [Solidity Documentation](https://docs.soliditylang.org/)

#### Step 2: Write Foundry Tests

Create comprehensive tests in `packages/contracts/test/` to validate contract behavior before deployment.

**File: `packages/contracts/test/PaymentChannel.t.sol`**

```solidity
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/PaymentChannel.sol";

contract PaymentChannelTest is Test {
    PaymentChannel public channel;
    address public sender = address(0x1);
    address public recipient = address(0x2);
    uint256 public duration = 3600; // 1 hour

    function setUp() public {
        // Fund sender account for testing
        vm.deal(sender, 10 ether);

        // Deploy payment channel as sender
        vm.prank(sender);
        channel = new PaymentChannel{value: 1 ether}(recipient, duration);
    }

    function testChannelCreation() public {
        assertEq(channel.sender(), sender);
        assertEq(channel.recipient(), recipient);
        assertEq(channel.getBalance(), 1 ether);
        assertTrue(channel.expiresAt() > block.timestamp);
    }

    function testChannelCreatedEvent() public {
        vm.expectEmit(true, true, false, true);
        emit PaymentChannel.ChannelCreated(sender, recipient, block.timestamp + duration);

        vm.prank(sender);
        new PaymentChannel{value: 1 ether}(recipient, duration);
    }

    function testFailZeroFunding() public {
        vm.prank(sender);
        new PaymentChannel{value: 0}(recipient, duration);
    }
}
```

**Test Structure Explanation:**

- **setUp()**: Runs before each test function (deploy contract, fund accounts)
- **Assertions**: `assertEq()`, `assertTrue()` validate expected outcomes
- **Cheat codes**: `vm.prank()` sets msg.sender, `vm.deal()` funds accounts
- **Event testing**: `vm.expectEmit()` validates event emissions
- **Failure tests**: `testFail*` prefix expects function to revert

#### Step 3: Run Tests Against Local Anvil

Execute tests against your local Anvil node to validate contract logic.

**Command:**

```bash
forge test --fork-url http://localhost:8545
```

**Expected Output:**

```
[⠢] Compiling...
[⠆] Compiling 2 files with 0.8.20
[⠰] Solc 0.8.20 finished in 1.23s
Compiler run successful!

Running 3 tests for test/PaymentChannel.t.sol:PaymentChannelTest
[PASS] testChannelCreatedEvent() (gas: 89234)
[PASS] testChannelCreation() (gas: 56782)
[PASS] testFailZeroFunding() (gas: 12345)
Test result: ok. 3 passed; 0 failed; finished in 2.34ms
```

**Debugging Test Failures:**

If tests fail, use `-vvvv` flag for detailed traces:

```bash
forge test --fork-url http://localhost:8545 -vvvv
```

This shows:

- Full transaction traces
- State changes
- Revert reasons with exact line numbers

#### Step 4: Deploy to Local Anvil

Deploy your tested contract to Anvil for integration testing with connectors.

**File: `packages/contracts/script/Deploy.s.sol`**

```solidity
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../src/PaymentChannel.sol";

contract DeployScript is Script {
    function run() public {
        vm.startBroadcast();

        address recipient = 0x70997970C51812dc3A010C7d01b50e0d17dc79C8;
        uint256 duration = 86400; // 24 hours

        PaymentChannel channel = new PaymentChannel{value: 1 ether}(recipient, duration);
        console.log("PaymentChannel deployed at:", address(channel));
        console.log("Sender:", channel.sender());
        console.log("Recipient:", channel.recipient());
        console.log("Balance:", channel.getBalance());

        vm.stopBroadcast();
    }
}
```

**Deploy Command:**

```bash
forge script script/Deploy.s.sol \
  --rpc-url http://localhost:8545 \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
  --broadcast
```

**Expected Output:**

```
[⠢] Compiling...
No files changed, compilation skipped

Script ran successfully.
Gas used: 234567

== Logs ==
  PaymentChannel deployed at: 0x5FbDB2315678afecb367f032d93F642f64180aa3
  Sender: 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266
  Recipient: 0x70997970C51812dc3A010C7d01b50e0d17dc79C8
  Balance: 1000000000000000000

ONCHAIN EXECUTION COMPLETE & SUCCESSFUL.
```

**Capture the deployed contract address** (`0x5FbDB2315678afecb367f032d93F642f64180aa3`) for next steps.

**Verify Deployment:**

```bash
cast code 0x5FbDB2315678afecb367f032d93F642f64180aa3 --rpc-url http://localhost:8545
```

Expected: Long bytecode hex string (contract deployed successfully)

#### Step 5: Test Integration with Connectors

Configure your M2M connector to interact with the deployed contract and verify payment channel functionality.

**Update connector config** (example: `packages/connector/config/development.yml`):

```yaml
blockchain:
  type: evm
  rpc_url: http://anvil:8545
  payment_channel_address: '0x5FbDB2315678afecb367f032d93F642f64180aa3'
```

**Restart connector:**

```bash
make dev-reset
```

**Send test ILP packet:**

```bash
# Example using test packet sender
npm run test:send-packet --connector=connector-a --amount=100
```

**Monitor Anvil logs** for contract interactions:

```bash
docker logs -f anvil_base_local
```

Expected: Transaction logs showing contract function calls, events emitted, state changes.

#### Step 6: Deploy to Base Sepolia Testnet

After successful local testing, deploy to Base Sepolia testnet for production-like validation.

**Update environment variables** in `.env.testnet`:

```bash
BASE_SEPOLIA_RPC_URL=https://sepolia.base.org
# Or use Alchemy/Infura for better reliability
BASE_SEPOLIA_RPC_URL=https://base-sepolia.g.alchemy.com/v2/YOUR_API_KEY
```

**Deploy Command:**

```bash
forge script script/Deploy.s.sol \
  --rpc-url $BASE_SEPOLIA_RPC_URL \
  --private-key $TESTNET_PRIVATE_KEY \
  --broadcast \
  --verify
```

**Flags Explained:**

- `--broadcast`: Actually submit transactions (omit for dry-run)
- `--verify`: Auto-verify contract on BaseScan (Etherscan for Base)

**Expected Output:**

```
[⠢] Compiling...
Script ran successfully.

ONCHAIN EXECUTION COMPLETE & SUCCESSFUL.

Contract deployed at: 0xABCD1234...
Waiting for confirmations...
Verified contract on BaseScan: https://sepolia.basescan.org/address/0xABCD1234...
```

**Wait for confirmations**: Base Sepolia has ~2-second block time, but wait for 2-3 blocks before testing.

#### Step 7: Run Integration Tests on Testnet

Validate that your contract behaves identically on testnet as it did on Anvil.

**Update test configuration** to use public Base Sepolia endpoint:

```bash
# In test config or environment
export RPC_URL=https://sepolia.base.org
export CONTRACT_ADDRESS=0xABCD1234...
```

**Run smoke tests:**

```bash
forge test --fork-url $RPC_URL --match-test testChannelCreation
```

**Verify contract behavior:**

```bash
# Check contract balance
cast call $CONTRACT_ADDRESS "getBalance()" --rpc-url $RPC_URL

# Check sender address
cast call $CONTRACT_ADDRESS "sender()" --rpc-url $RPC_URL
```

**Important**: Testnet transactions cost real (testnet) ETH and have gas fees. Monitor gas costs and ensure testnet ETH balance is sufficient.

**Get testnet ETH**: [Base Sepolia Faucet](https://www.coinbase.com/faucets/base-ethereum-sepolia-faucet)

#### Step 8: Security Audit

Before production deployment, perform comprehensive security review and gas optimization.

**Static Analysis with Slither:**

```bash
slither packages/contracts/src/PaymentChannel.sol
```

Expected output: List of potential vulnerabilities (high/medium/low severity)

**Address all HIGH and MEDIUM severity findings** before mainnet deployment.

**Gas Optimization Review:**

```bash
# Generate gas snapshot
forge snapshot

# Review gas usage per function
forge test --gas-report
```

**Optimization targets:**

- Storage layout (minimize SSTORE operations)
- Function visibility (use external instead of public where possible)
- Data types (use uint256 instead of smaller types for gas efficiency)

**External Audit (for production contracts):**

- Engage professional security auditors (OpenZeppelin, Trail of Bits, Consensys Diligence)
- Budget: $10k-$50k+ depending on contract complexity
- Timeline: 2-4 weeks for comprehensive audit

#### Step 9: Deploy to Base Mainnet (Production)

After testnet validation and security audit, deploy to Base mainnet for production use.

**Update environment:**

```bash
BASE_MAINNET_RPC_URL=https://mainnet.base.org
# Or use paid RPC provider for better reliability
BASE_MAINNET_RPC_URL=https://base-mainnet.g.alchemy.com/v2/YOUR_API_KEY
```

**Deploy Command:**

```bash
forge script script/Deploy.s.sol \
  --rpc-url $BASE_MAINNET_RPC_URL \
  --private-key $MAINNET_PRIVATE_KEY \
  --broadcast \
  --verify
```

**CRITICAL SECURITY:**

- Use hardware wallet or secure key management (never paste private keys in terminal history)
- Verify deployment transaction before confirming
- Use `--slow` flag for lower gas prices if not time-sensitive

**Post-Deployment Steps:**

1. **Verify contract on BaseScan:**

   ```bash
   # Verify at: https://basescan.org/address/<contract-address>
   ```

2. **Update production connector config:**

   ```yaml
   blockchain:
     rpc_url: https://mainnet.base.org
     payment_channel_address: '0xMAINNET_CONTRACT_ADDRESS'
   ```

3. **Monitor mainnet deployment for 24 hours:**
   - Watch for unexpected transactions
   - Monitor gas usage
   - Validate event emissions
   - Test with small amounts first

4. **Gradual rollout:**
   - Start with 1% of traffic
   - Monitor for 24 hours
   - Increase to 10%, 50%, 100% over 1 week

#### Common Pitfalls and Tips

**Pitfall 1: Deploying without testing on Anvil first**

- **Always test locally first**: Instant feedback, zero gas costs, unlimited iterations
- Anvil catches 90% of issues before they reach testnet

**Pitfall 2: Reusing private keys across environments**

- **Use separate keys for dev/testnet/mainnet**: NEVER reuse
- Development key can be committed to repo (test funds only)
- Testnet/mainnet keys must be secured (hardware wallet, environment variables)

**Pitfall 3: Skipping Etherscan verification**

- **Verify immediately after deployment**: Enables public contract interaction
- Use `--verify` flag or verify manually on BaseScan
- Verified contracts build trust and enable debugging

**Pitfall 4: Not keeping deployment scripts in version control**

- **Commit deployment scripts**: Reproducible deployments
- Document deployment parameters (recipient, duration, initial funding)
- Tag git commits for production deployments

**Tip: Use Make targets for common workflows**

Create `Makefile` shortcuts:

```makefile
deploy-local:
	forge script script/Deploy.s.sol --rpc-url http://localhost:8545 --broadcast

deploy-testnet:
	forge script script/Deploy.s.sol --rpc-url $(BASE_SEPOLIA_RPC_URL) --broadcast --verify

test-contracts:
	forge test --fork-url http://localhost:8545 -vv
```

### Debugging Workflows

#### Debugging Smart Contract Issues

**Use Foundry's verbose trace flags** for detailed execution inspection:

**Level 1: Basic logs (-v)**

```bash
forge test -v
```

Shows test results and console.log() outputs.

**Level 2: Event logs (-vv)**

```bash
forge test -vv
```

Shows test results, logs, and emitted events.

**Level 3: Failed test traces (-vvv)**

```bash
forge test -vvv
```

Shows stack traces for failed tests (most useful).

**Level 4: All test traces (-vvvv)**

```bash
forge test -vvvv
```

Shows complete stack traces for ALL tests (very verbose).

**Check Anvil logs** for transaction details:

```bash
docker logs anvil_base_local
```

Shows: Transaction hashes, block numbers, gas used, contract deployments, function calls.

**Query contract state** with cast:

```bash
# Check balance
cast balance <contract-address> --rpc-url http://localhost:8545

# Call view function
cast call <contract-address> "getBalance()" --rpc-url http://localhost:8545

# Call with parameters
cast call <contract-address> "balanceOf(address)" <wallet-address> --rpc-url http://localhost:8545
```

**Foundry debugger** for interactive debugging:

```bash
forge test --debug testFunctionName
```

Opens interactive TUI debugger with:

- Step through execution line-by-line
- Inspect stack, memory, storage
- View opcode execution
- Identify exact revert location

#### Common Debugging Scenarios

**Scenario 1: "Transaction reverted" (Solidity)**

**Symptom**: forge script or cast send fails with generic revert

**Debug Steps**:

1. Run with `-vvvv` to see revert reason:

   ```bash
   forge script Deploy.s.sol -vvvv --rpc-url http://localhost:8545
   ```

2. Check require() conditions in contract:

   ```solidity
   require(msg.value > 0, "Must fund channel"); // ← Revert reason here
   ```

3. Verify function parameters match expected types
4. Check account balance sufficient for transaction + gas

**Scenario 2: "Insufficient funds" (Anvil)**

**Symptom**: Transaction fails with out-of-gas or insufficient balance

**Debug Steps**:

1. Check account balance:

   ```bash
   cast balance <account> --rpc-url http://localhost:8545
   ```

2. Verify gas estimation:

   ```bash
   cast estimate <contract-address> "functionName()" --rpc-url http://localhost:8545
   ```

3. Fund account if needed (Anvil pre-funded accounts should have 10000 ETH)

## Deploying Your First Smart Contract

### Prerequisites

Before deploying your first smart contract, ensure you have:

- **Foundry installed locally**: [Installation guide](https://book.getfoundry.sh/getting-started/installation)
- **Anvil running**: See [Quick Start](#quick-start-5-minutes) section
- **Basic Solidity knowledge**: Familiarity with contract structure and syntax

### Step 1: Install Foundry

Install Foundry toolchain (forge, cast, anvil) on your local machine:

**Command:**

```bash
curl -L https://foundry.paradigm.xyz | bash
foundryup
```

**Verify installation:**

```bash
forge --version
```

**Expected Output:**

```
forge 0.2.0 (abc123 2024-01-15T00:00:00.000000000Z)
```

**Troubleshooting**: If `foundryup` not found, restart terminal or add to PATH:

```bash
export PATH="$HOME/.foundry/bin:$PATH"
```

### Step 2: Create New Foundry Project

Initialize a new Foundry project for your smart contracts:

**Command:**

```bash
forge init packages/contracts
cd packages/contracts
```

**Project Structure Created:**

```
packages/contracts/
├── src/              # Smart contract source files
├── test/             # Test files
├── script/           # Deployment scripts
├── lib/              # Dependencies (forge-std)
└── foundry.toml      # Foundry configuration
```

**Explanation**:

- **src/**: Where you write Solidity contracts
- **test/**: Co-located tests for each contract
- **script/**: Deployment and interaction scripts
- **lib/**: Dependencies installed via `forge install`

### Step 3: Write a Simple Storage Contract

Create a minimal smart contract to demonstrate deployment workflow.

**File: `packages/contracts/src/SimpleStorage.sol`**

```solidity
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @title SimpleStorage
/// @notice Stores and retrieves a single uint256 value
contract SimpleStorage {
    uint256 private storedValue;

    /// @notice Emitted when stored value changes
    event ValueChanged(uint256 newValue);

    /// @notice Set the stored value
    /// @param _value New value to store
    function setValue(uint256 _value) public {
        storedValue = _value;
        emit ValueChanged(_value);
    }

    /// @notice Get the current stored value
    /// @return The stored value
    function getValue() public view returns (uint256) {
        return storedValue;
    }
}
```

**Contract Explanation:**

- **State Variable**: `storedValue` persists between function calls (stored on blockchain)
- **Setter Function**: `setValue()` modifies state and emits event
- **Getter Function**: `getValue()` reads state (view function, no gas cost for external calls)
- **Event**: `ValueChanged` logs state changes for off-chain indexing

### Step 4: Write a Test for the Contract

Validate contract behavior with comprehensive tests before deployment.

**File: `packages/contracts/test/SimpleStorage.t.sol`**

```solidity
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/SimpleStorage.sol";

contract SimpleStorageTest is Test {
    SimpleStorage public simpleStorage;

    function setUp() public {
        simpleStorage = new SimpleStorage();
    }

    function testSetValue() public {
        simpleStorage.setValue(42);
        assertEq(simpleStorage.getValue(), 42);
    }

    function testValueChangedEvent() public {
        vm.expectEmit(true, true, true, true);
        emit SimpleStorage.ValueChanged(100);
        simpleStorage.setValue(100);
    }

    function testInitialValueIsZero() public {
        assertEq(simpleStorage.getValue(), 0);
    }
}
```

**Test Explanation:**

- **setUp()**: Deploys fresh contract before each test
- **testSetValue()**: Verifies setValue() updates state correctly
- **testValueChangedEvent()**: Validates event emission
- **Assertions**: `assertEq()` compares expected vs actual values

### Step 5: Run Tests Locally

Execute tests against local Anvil to validate contract logic.

**Command:**

```bash
forge test --fork-url http://localhost:8545
```

**Expected Output:**

```
[⠢] Compiling...
[⠆] Compiling 3 files with 0.8.20
[⠰] Solc 0.8.20 finished in 823ms
Compiler run successful!

Running 3 tests for test/SimpleStorage.t.sol:SimpleStorageTest
[PASS] testInitialValueIsZero() (gas: 8234)
[PASS] testSetValue() (gas: 29876)
[PASS] testValueChangedEvent() (gas: 31245)
Test result: ok. 3 passed; 0 failed; finished in 1.45ms
```

**All tests passing** (green checkmarks) indicates contract ready for deployment.

**If tests fail:**

1. Review error messages for revert reasons
2. Use `-vvv` flag for detailed traces:

   ```bash
   forge test --fork-url http://localhost:8545 -vvv
   ```

3. Fix contract or test code and re-run

### Step 6: Create Deployment Script

Write deployment script to automate contract deployment.

**File: `packages/contracts/script/Deploy.s.sol`**

```solidity
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../src/SimpleStorage.sol";

contract DeployScript is Script {
    function run() public {
        vm.startBroadcast();

        SimpleStorage simpleStorage = new SimpleStorage();
        console.log("SimpleStorage deployed at:", address(simpleStorage));

        // Optional: Initialize with value
        simpleStorage.setValue(42);
        console.log("Initial value set to:", simpleStorage.getValue());

        vm.stopBroadcast();
    }
}
```

**Script Explanation:**

- **vm.startBroadcast()**: Begin recording transactions for broadcast
- **new SimpleStorage()**: Deploy contract
- **console.log()**: Output deployment info
- **vm.stopBroadcast()**: Stop recording transactions

### Step 7: Deploy to Local Anvil

Deploy contract to your local Anvil node.

**Command:**

```bash
forge script script/Deploy.s.sol \
  --rpc-url http://localhost:8545 \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
  --broadcast
```

**Expected Output:**

```
[⠢] Compiling...
No files changed, compilation skipped

Script ran successfully.
Gas used: 145623

== Logs ==
  SimpleStorage deployed at: 0x5FbDB2315678afecb367f032d93F642f64180aa3
  Initial value set to: 42

ONCHAIN EXECUTION COMPLETE & SUCCESSFUL.
Total Paid: 0.000145623 ETH (145623 gas * 1 gwei)
```

**Save the deployed contract address**: `0x5FbDB2315678afecb367f032d93F642f64180aa3`

**Verify deployment** succeeded:

```bash
cast code 0x5FbDB2315678afecb367f032d93F642f64180aa3 --rpc-url http://localhost:8545
```

**Expected**: Long bytecode hex string starting with `0x608060...` (contract deployed)

**If empty (`0x`)**: Deployment failed, check error messages

### Step 8: Interact with Deployed Contract

Test contract functionality by calling functions directly.

**Set value to 123:**

```bash
cast send 0x5FbDB2315678afecb367f032d93F642f64180aa3 \
  "setValue(uint256)" 123 \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
  --rpc-url http://localhost:8545
```

**Expected Output:**

```
blockHash               0xabcdef1234567890...
blockNumber             20702369
transactionHash         0x1234567890abcdef...
status                  1 (success)
```

**Get value:**

```bash
cast call 0x5FbDB2315678afecb367f032d93F642f64180aa3 \
  "getValue()" \
  --rpc-url http://localhost:8545
```

**Expected Output:**

```
0x000000000000000000000000000000000000000000000000000000000000007b
```

**Decode hex output**: `0x7b` = 123 in decimal (matches value we set)

**Decode using cast:**

```bash
cast --to-dec 0x000000000000000000000000000000000000000000000000000000000000007b
```

Output: `123`

### Step 9: Verify Deployment with Connector Integration (Optional)

Integrate deployed contract with M2M connector for full-stack testing.

**Update connector configuration** (`packages/connector/config/development.yml`):

```yaml
blockchain:
  type: evm
  rpc_url: http://anvil:8545
  simple_storage_address: '0x5FbDB2315678afecb367f032d93F642f64180aa3'
```

**Restart connector:**

```bash
make dev-reset
```

**Send test ILP packet** (if connector configured to interact with SimpleStorage):

```bash
npm run test:send-packet --connector=connector-a --amount=100
```

**Verify contract interaction** in Anvil logs:

```bash
docker logs -f anvil_base_local | grep setValue
```

Expected: Transaction logs showing `setValue()` function calls from connector.

## Troubleshooting

### Issue: Anvil won't start

**Symptoms:**

- Docker container fails to start
- Health check never passes
- Container logs show errors

**Problem:** Port 8545 already in use by another process

**Solution:**

1. Check what's using port 8545:

   ```bash
   lsof -i :8545
   ```

2. Kill the conflicting process:

   ```bash
   kill -9 <PID>
   ```

3. Or change Anvil port in `docker-compose-dev.yml`:
   ```yaml
   ports:
     - '8546:8545' # Map host port 8546 to container port 8545
   ```

### Issue: Anvil fork fails to download

**Symptoms:**

- Container starts but health check fails
- Logs show "fork download timeout" or rate limit errors
- Fork download takes 5+ minutes

**Problem:** `BASE_SEPOLIA_RPC_URL` rate limited or down

**Solution:**

Configure an alternative RPC endpoint in `.env.dev`:

```bash
# Use Alchemy (requires free API key)
BASE_SEPOLIA_RPC_URL=https://base-sepolia.g.alchemy.com/v2/YOUR_API_KEY

# Or use Tenderly (free tier)
BASE_SEPOLIA_RPC_URL=https://base-sepolia.gateway.tenderly.co

# Or use Infura (requires free project ID)
BASE_SEPOLIA_RPC_URL=https://base-sepolia.infura.io/v3/YOUR_PROJECT_ID
```

Restart Anvil:

```bash
docker-compose -f docker-compose-dev.yml restart anvil
```

### Issue: Forked state is outdated

**Symptoms:**

- Missing recent Base Sepolia contracts or state
- Fork block number is weeks/months old
- Need newer testnet state for testing

**Problem:** `FORK_BLOCK_NUMBER` is too old

**Solution:**

1. Get the latest Base Sepolia block number:

   ```bash
   curl https://sepolia.base.org -X POST \
     -H 'Content-Type: application/json' \
     --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}'
   ```

2. Convert hex result to decimal:

   ```bash
   # Example: "0x13c377f" → 20702367
   echo $((16#13c377f))
   ```

3. Update `FORK_BLOCK_NUMBER` in `.env.dev`:

   ```bash
   FORK_BLOCK_NUMBER=20702367  # Replace with latest block
   ```

4. Restart Anvil:
   ```bash
   docker-compose -f docker-compose-dev.yml restart anvil
   ```

### Issue: Smart contract deployment fails

**Symptoms:**

- `forge create` or MetaMask transactions fail
- Error: "invalid chain id" or "network mismatch"
- Contract deploys but doesn't behave correctly

**Problem:** Using wrong chain ID or RPC URL

**Solution:**

Verify configuration:

1. **Chain ID must be 84532** (Base Sepolia)
2. **RPC URL must be `http://localhost:8545`** (or `http://anvil:8545` from containers)
3. **Use `--optimism` flag** when starting Anvil (already configured in docker-compose-dev.yml)

Check Anvil is using correct chain ID:

```bash
curl -X POST http://localhost:8545 \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}'
```

Expected result: `{"result":"0x14a34"}` (84532 in hex)

### Issue: Smart contract deployment fails with 'invalid chain id'

**Symptoms:**

- `forge script` fails with chain ID mismatch error
- Error message: "invalid chain id" or "expected X, got Y"

**Problem:** Anvil not started with --optimism flag or wrong chain ID configured

**Solution:**

1. Verify Anvil started with correct flags in `docker-compose-dev.yml`:

   ```yaml
   command:
     - anvil
     - --fork-url=${BASE_SEPOLIA_RPC_URL}
     - --fork-block-number=${FORK_BLOCK_NUMBER}
     - --chain-id=84532
     - --optimism
   ```

2. Check current chain ID:

   ```bash
   curl -X POST http://localhost:8545 \
     -H 'Content-Type: application/json' \
     --data '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}'
   ```

   **Expected**: `{"result":"0x14a34"}` (84532 in hex)

3. If wrong chain ID, restart Anvil:
   ```bash
   make dev-reset
   ```

### Issue: Contract deployment succeeds but contract doesn't work

**Symptoms:**

- Contract deployed successfully but function calls revert
- Contract returns unexpected values or reverts with no reason
- Previously working contract suddenly stops functioning

**Problem:** Contract state not persisted (Anvil restarted) or using wrong RPC URL

**Solution:**

1. Verify Anvil still running:

   ```bash
   docker ps | grep anvil
   ```

2. Check contract bytecode exists at deployment address:

   ```bash
   cast code <contract-address> --rpc-url http://localhost:8545
   ```

   **Expected**: Long hex string starting with `0x608060...`
   **If `0x`**: Contract doesn't exist, Anvil was reset

3. Redeploy contract if Anvil was reset:
   ```bash
   forge script script/Deploy.s.sol --rpc-url http://localhost:8545 --broadcast
   ```

**Note**: Anvil uses ephemeral storage by default. All state is lost on container restart. This is intentional for clean development environment.

### Issue: Gas estimation fails with 'execution reverted'

**Symptoms:**

- `forge script` or `cast send` fails during gas estimation phase
- Error: "execution reverted" before transaction is submitted
- Transaction would fail if submitted

**Problem:** Contract require() condition failing or insufficient account funds

**Solution:**

1. Run with `-vvvv` flag to see revert reason:

   ```bash
   forge script Deploy.s.sol -vvvv --rpc-url http://localhost:8545
   ```

   Output shows exact require() condition that failed.

2. Check account balance sufficient for transaction + gas:

   ```bash
   cast balance <account> --rpc-url http://localhost:8545
   ```

   Expected: At least 1 ETH (Anvil pre-funded accounts have 10000 ETH)

3. Review contract require() conditions:

   ```solidity
   require(msg.value > 0, "Must fund channel"); // ← Check this condition
   require(recipient != address(0), "Invalid recipient"); // ← And this
   ```

4. Verify function parameters match expected types and values

## FAQ

### Q: Why use Anvil instead of Hardhat?

**A:** Anvil is 2-3x faster for testing, has better Foundry integration, and supports instant mining. Hardhat is also a great tool, but Anvil is optimized for the Foundry toolchain used in this project.

### Q: Can I use public Base Sepolia testnet instead of Anvil?

**A:** Yes, but local Anvil provides:

- **Faster iteration**: Instant block confirmation vs 2-second Base L2 block time
- **No rate limits**: Unlimited RPC requests vs rate-limited public endpoints
- **Offline development**: Work without internet after initial fork download
- **Deterministic state**: Same state across all developer machines

Use public Base Sepolia for final testing before mainnet deployment.

### Q: Do I need to run Anvil for all development?

**A:** Only if working on **EVM Payment Channels** smart contracts. If you're working on ILP connectors, telemetry, or dashboard, Anvil is not required.

### Q: How much disk space does Anvil use?

**A:** Approximately **2-5GB** for Base Sepolia fork (state download at fork block). Anvil uses ephemeral storage by default—state is cleared on container restart.

### Q: Can I persist Anvil state across container restarts?

**A:** Anvil uses ephemeral storage intentionally for clean development environment. If you need persistent state, add a Docker volume:

```yaml
# In docker-compose-dev.yml (not recommended for most use cases)
anvil:
  volumes:
    - anvil-data:/root/.anvil
```

### Q: How often should I update the fork block number?

**A:** Update `FORK_BLOCK_NUMBER` every **1-2 weeks** to get recent Base Sepolia state. Older fork blocks work fine but may miss recent contract deployments or state changes.

### Q: Can I fork Base mainnet instead of Base Sepolia?

**A:** Yes, but it's not recommended for development:

- **Base mainnet**: ~50GB+ state size, slower fork download
- **Base Sepolia**: ~2-5GB state size, faster fork download

Change `BASE_SEPOLIA_RPC_URL` to `https://mainnet.base.org` and update `FORK_BLOCK_NUMBER` to a recent Base mainnet block if needed for production testing.

### Q: What's the difference between Base Sepolia testnet and Base mainnet?

**A:** Base Sepolia (testnet) is recommended for development for several reasons:

**Base Sepolia (testnet) benefits:**

- **Free testnet ETH** from faucets (no real money required)
- **2-second block time** (same as mainnet, realistic testing)
- **Smaller state size** (~5GB vs 50GB+ for mainnet fork)
- **Safe for mistakes** (no financial risk if something goes wrong)
- **Identical behavior** to mainnet (same OP Stack configuration)

**Base mainnet (production):**

- Use **only after** testnet validation and security audit
- Real ETH required for gas fees (financial risk)
- Larger state size (slower fork downloads)
- Production-grade monitoring required

**Anvil can fork either**, but testnet is recommended for cost and safety during development.

### Q: Should I use Anvil or public Base Sepolia RPC for development?

**A:** Use **Anvil for rapid local development**, switch to **public Base Sepolia for integration testing**:

**Use Anvil for local development:**

- **Instant block confirmation** (no 2-second wait)
- **Unlimited RPC requests** (no rate limits)
- **Offline development** after initial fork download
- **Zero gas costs** (unlimited experimentation)
- **Deterministic state** (reproducible across machines)

**Use public Base Sepolia for integration testing:**

- **Realistic network conditions** (block time, gas estimation, network latency)
- **External visibility** (team members can verify deployments on BaseScan)
- **Persistence** (contracts remain deployed across sessions)
- **Production-like environment** (catches network-specific issues)

**Best Practice**: Develop and test on Anvil (fast iteration), then deploy to Base Sepolia for final validation before mainnet.

### Q: Can I use Hardhat instead of Foundry/Anvil?

**A:** Foundry/Anvil is the recommended and supported toolchain for M2M because:

**Foundry/Anvil advantages:**

- **2-3x faster test execution** (native Rust vs JavaScript/TypeScript)
- **Better gas optimization tooling** (`forge snapshot` for gas analysis)
- **Native Solidity test framework** (no JavaScript/TypeScript needed)
- **Excellent Base L2 / OP Stack support** with `--optimism` flag
- **Built-in fuzzing and invariant testing**
- **Faster compilation** (Rust compiler)

**A:** Hardhat is also excellent and can work, but:

- M2M documentation assumes Foundry
- Hardhat requires JavaScript/TypeScript for tests
- Slightly slower test execution
- Additional configuration for OP Stack compatibility

If you prefer Hardhat, it will work, but you'll need to adapt documentation examples.

### Q: What happens if I restart Anvil? Do I lose deployed contracts?

**A:** YES - Anvil uses **ephemeral storage by default**:

**On Anvil restart:**

- **All deployed contracts lost** (addresses become empty)
- **Forked state re-downloaded** from BASE_SEPOLIA_RPC_URL (reverts to fork block)
- **Account balances reset** to default (10000 ETH per test account)
- **Transaction history cleared** (fresh chain)

**This is intentional** for clean development environment. Every restart gives you a pristine fork.

**Workaround for persistent storage** (not recommended for most use cases):

Add volume in `docker-compose-dev.yml`:

```yaml
anvil:
  volumes:
    - anvil-data:/root/.anvil
```

**Best Practice**: Accept ephemeral storage. Redeploy contracts quickly using `forge script` after restart.

### Q: How do I debug smart contract reverts on Anvil?

**A:** Foundry provides excellent debugging tools for contract reverts:

**Step 1: Run tests with -vvvv flag for detailed traces**

```bash
forge test -vvvv --match-test testFunctionName
```

Shows:

- Full transaction traces
- State changes
- Revert reasons with exact line numbers
- Stack traces

**Step 2: Use Foundry debugger for interactive debugging**

```bash
forge test --debug testFunctionName
```

Opens interactive TUI debugger:

- Step through execution line-by-line
- Inspect stack, memory, storage at each step
- View opcode execution
- Identify exact revert location

**Step 3: Check Anvil logs for transaction details**

```bash
docker logs anvil_base_local
```

Shows transaction hashes, gas used, contract events.

**Step 4: Query contract state with cast**

```bash
# Check specific state variable
cast call <address> "storedValue()" --rpc-url http://localhost:8545

# Check balance
cast balance <address> --rpc-url http://localhost:8545
```

**Common revert reasons:**

- Insufficient funds (account balance too low)
- Failed require() conditions (check contract logic)
- Wrong function parameters (type mismatch)
- Gas estimation failure (contract logic error)

### Q: How do I find recent Base Sepolia block numbers for forking?

**A:** Query Base Sepolia RPC for current block number:

**Step 1: Get current block (hex format)**

```bash
curl https://sepolia.base.org -X POST \
  -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}'
```

**Expected Response:**

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": "0x13c377f"
}
```

**Step 2: Convert hex to decimal**

```bash
echo $((16#13c377f))
```

**Output:** `20702367` (decimal block number)

**Step 3: Update FORK_BLOCK_NUMBER in .env.dev**

```bash
FORK_BLOCK_NUMBER=20702367
```

**Step 4: Restart Anvil to apply new fork block**

```bash
make dev-reset
```

**Best Practice**: Update fork block every **1-2 weeks** to track recent testnet state. Older fork blocks work fine but may miss recent contract deployments or state changes.

**Alternative**: Use BaseScan to find specific block by timestamp:

[BaseScan Sepolia Blocks](https://sepolia.basescan.org/blocks)

### Q: Can I use MetaMask with Anvil?

**A:** Yes, MetaMask supports custom Ethereum networks:

1. Open MetaMask → Networks → Add Network Manually
2. Enter network details:
   - **Network Name**: Anvil Local (Base Sepolia Fork)
   - **RPC URL**: `http://localhost:8545`
   - **Chain ID**: `84532` (Base Sepolia)
   - **Currency Symbol**: ETH
3. Click "Save"
4. Import test account using Anvil private key:
   - **Private Key**: `0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80` (Account #0)
5. Send transactions and deploy contracts via MetaMask

## Environment Variable Reference

### Overview

Environment variables control blockchain node configuration, connector behavior, and development tooling. Understanding variable precedence and application ensures predictable development environments.

**Variable Precedence (highest to lowest):**

1. **Shell environment variables**: `export BASE_SEPOLIA_RPC_URL=https://custom.url`
2. **.env.dev file**: `BASE_SEPOLIA_RPC_URL=https://sepolia.base.org`
3. **Docker Compose defaults**: `${BASE_SEPOLIA_RPC_URL:-https://sepolia.base.org}`

**When variables are loaded:**

- Container startup only (no hot-reload)
- Restart containers to apply changes: `make dev-reset` or `docker-compose restart`

**Verifying active values:**

```bash
docker exec <container-name> env | grep <VAR_NAME>
```

### Anvil Configuration Variables

Configure Anvil's blockchain forking and network behavior.

| Variable             | Default                  | Description                                          | Required | Example                                        |
| -------------------- | ------------------------ | ---------------------------------------------------- | -------- | ---------------------------------------------- |
| BASE_SEPOLIA_RPC_URL | https://sepolia.base.org | RPC endpoint for forking Base Sepolia state          | No       | https://base-sepolia.g.alchemy.com/v2/YOUR_KEY |
| FORK_BLOCK_NUMBER    | 20702367                 | Block number to fork from (pin for consistent state) | No       | 21000000                                       |

**Impact of changing variables:**

- **BASE_SEPOLIA_RPC_URL**: Different RPC provider affects fork download speed and rate limits
  - Public endpoint (sepolia.base.org): Free, rate-limited, slower
  - Alchemy/Infura: Free tier available, faster, higher rate limits
  - Update every 1-2 weeks to track recent testnet state

- **FORK_BLOCK_NUMBER**: Older blocks may miss recent contract deployments
  - Find recent block: `curl https://sepolia.base.org -X POST -H 'Content-Type: application/json' --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}'`
  - Convert hex to decimal: `echo $((16#<hex-result>))`

**Link to Base Sepolia block explorer**: [BaseScan Sepolia](https://sepolia.basescan.org/)

### Connector Configuration Variables

Control connector logging, development features, and dashboard integration.

| Variable          | Default     | Description                                           | Required | Example    |
| ----------------- | ----------- | ----------------------------------------------------- | -------- | ---------- |
| LOG_LEVEL         | info        | Logging verbosity (debug, info, warn, error)          | No       | debug      |
| NODE_ENV          | development | Environment mode (enables hot-reload, debug features) | No       | production |
| DASHBOARD_ENABLED | false       | Enable dashboard telemetry emission                   | No       | true       |
| ENABLE_HOT_RELOAD | true        | Auto-restart connectors on code changes               | No       | false      |
| AUTO_RESTART      | true        | Restart connectors on crash during development        | No       | false      |

**Impact of changing variables:**

- **LOG_LEVEL**:
  - `debug`: Verbose logging (packet details, routing decisions, state changes)
  - `info`: Standard logging (connection events, transactions)
  - `warn`: Warnings only (potential issues)
  - `error`: Errors only (failures)
  - Use `debug` for development, `info` for production

- **ENABLE_HOT_RELOAD**:
  - `true`: Connectors auto-restart when source files change (faster development)
  - `false`: Manual restart required (use for performance testing, profiling)
  - Requires `nodemon` or similar file watcher

- **DASHBOARD_ENABLED**:
  - `true`: Connector emits telemetry to dashboard (visualize packet routing)
  - `false`: No telemetry emission (reduces overhead for testing)

### TigerBeetle Configuration Variables

Configure TigerBeetle distributed ledger behavior.

| Variable                  | Default | Description                                      | Required | Example              |
| ------------------------- | ------- | ------------------------------------------------ | -------- | -------------------- |
| TIGERBEETLE_CLUSTER_ID    | 0       | Unique cluster identifier (IMMUTABLE after init) | No       | 1                    |
| TIGERBEETLE_REPLICA_COUNT | 1       | Number of replicas (1=dev, 3-5=prod)             | No       | 3                    |
| TIGERBEETLE_PORT          | 3000    | Internal port (NOT exposed to host)              | No       | 3000                 |
| TIGERBEETLE_DATA_DIR      | /data   | Data directory inside container                  | No       | /var/lib/tigerbeetle |

**CRITICAL WARNING**: Changing `TIGERBEETLE_CLUSTER_ID` requires deleting volume (data loss).

**Impact of changing variables:**

- **TIGERBEETLE_CLUSTER_ID**:
  - Must be unique across all TigerBeetle clusters
  - **IMMUTABLE** after initialization (changing requires data wipe)
  - To change: `docker volume rm m2m_tigerbeetle_data && make dev-reset`

- **TIGERBEETLE_REPLICA_COUNT**:
  - `1` (development): Single replica, no fault tolerance
  - `3-5` (production): Quorum-based consensus, survives replica failures
  - Requires network coordination for multi-replica setups

### Network Mode Configuration (Testnet vs Local)

The `NETWORK_MODE` environment variable controls whether the test infrastructure connects to local Docker containers or public testnets. This is useful for ARM64 development (where some Docker images aren't available) or for production-like testing.

| Variable             | Default                  | Description                        | Required | Example                                        |
| -------------------- | ------------------------ | ---------------------------------- | -------- | ---------------------------------------------- |
| NETWORK_MODE         | local                    | Network mode: `local` or `testnet` | No       | testnet                                        |
| BASE_SEPOLIA_RPC_URL | https://sepolia.base.org | Base Sepolia RPC URL               | No       | https://base-sepolia.g.alchemy.com/v2/YOUR_KEY |

**Network Mode Behavior:**

- **`NETWORK_MODE=local` (default)**:
  - Connects to local Docker containers (Anvil)
  - Uses Docker-internal hostnames (e.g., `http://anvil:8545`)
  - Instant block confirmation times
  - Best for rapid development iteration

- **`NETWORK_MODE=testnet`**:
  - Connects to public testnet endpoints (Base Sepolia)
  - Real network latency (2-5 seconds for confirmations)
  - Best for ARM64 development or production-like testing

**Running Tests in Testnet Mode:**

```bash
# Run integration tests against public testnets
NETWORK_MODE=testnet npm run test:integration

# Run Docker agent tests against public testnets
NETWORK_MODE=testnet ./scripts/run-docker-agent-test.sh
```

**Timeout Adjustments:**

When using testnet mode, timeouts are automatically increased to accommodate network latency:

| Timeout Type     | Local Mode | Testnet Mode |
| ---------------- | ---------- | ------------ |
| Transaction Wait | 10 seconds | 60 seconds   |
| Health Check     | 30 seconds | 60 seconds   |
| HTTP Request     | 10 seconds | 30 seconds   |

**Faucet Rate Limits:**

- **Base Sepolia**: Use external faucets ([Coinbase faucet](https://www.coinbase.com/faucets/base-ethereum-sepolia-faucet), etc.)

**Example: ARM64 Development Workflow:**

ARM64 (Apple Silicon, Raspberry Pi) may not have Docker images for all services. Use testnet mode:

```bash
# In .env.dev
NETWORK_MODE=testnet

# Start only the services that work on ARM64
docker-compose -f docker-compose-dev.yml up -d anvil

# Run tests against public testnets for missing services
NETWORK_MODE=testnet npm test
```

### Production-Specific Variables

Variables used in production environments (from `.env.production.example`).

| Variable                   | Default    | Description                                      | Required       | Example                |
| -------------------------- | ---------- | ------------------------------------------------ | -------------- | ---------------------- |
| NODE_ID                    | (required) | Unique connector identifier                      | Yes            | production-connector-1 |
| BTP_PORT                   | 3000       | BTP server port (exposed to host in production)  | No             | 4000                   |
| DASHBOARD_TELEMETRY_URL    | (empty)    | Dashboard WebSocket URL (leave empty to disable) | No             | ws://dashboard:9000    |
| BTP*PEER*<PEER_ID>\_SECRET | (required) | BTP peer authentication secret                   | Yes (per peer) | <generated-secret>     |

**BTP Peer Secret Naming Pattern:**

- Peer ID → uppercase → `BTP_PEER_CONNECTOR_B_SECRET`
- Example: `connector-b` → `BTP_PEER_CONNECTOR_B_SECRET`
- Each peer connection requires unique secret

**Security Best Practices:**

- Generate secrets with `openssl rand -base64 32`
- Store secrets in secure vault (AWS Secrets Manager, HashiCorp Vault)
- Never commit secrets to version control
- Rotate secrets periodically (30-90 days)

**Reference**: See Story 7.5 for complete production deployment documentation.

### Changing Environment Variables

Follow these steps to safely update environment variables.

**Step 1: Edit .env.dev file**

```bash
# Edit with your preferred editor
vim .env.dev

# Or use sed for single variable
sed -i '' 's/LOG_LEVEL=info/LOG_LEVEL=debug/' .env.dev
```

**Step 2: Restart affected services**

For **Anvil**:

```bash
# Restart specific service
docker-compose -f docker-compose-dev.yml restart anvil
```

For **connectors**:

```bash
# Full reset ensures clean state
make dev-reset
```

For **TigerBeetle** (CRITICAL - changing cluster ID requires volume delete):

```bash
# ONLY if changing TIGERBEETLE_CLUSTER_ID
docker-compose -f docker-compose-dev.yml down
docker volume rm m2m_tigerbeetle_data
make dev-reset
```

**Step 3: Verify changes applied**

```bash
# Check container environment
docker exec <container-name> env | grep <VAR_NAME>

# Example: Verify Anvil fork block
docker exec anvil_base_local env | grep FORK_BLOCK_NUMBER
```

**Warning**: Some variables require container rebuild:

- `NODE_ENV`: Affects build process
- `ENABLE_HOT_RELOAD`: Requires different container command

Rebuild containers:

```bash
docker-compose -f docker-compose-dev.yml build
make dev-reset
```

## External Resources

### Anvil / Base L2 Resources

- **Foundry Documentation**: [https://book.getfoundry.sh/](https://book.getfoundry.sh/)
- **Anvil Reference**: [https://book.getfoundry.sh/reference/anvil/](https://book.getfoundry.sh/reference/anvil/)
- **Base Sepolia Documentation**: [https://docs.base.org/network-information](https://docs.base.org/network-information)
- **Base Sepolia Faucet**: [https://www.coinbase.com/faucets/base-ethereum-sepolia-faucet](https://www.coinbase.com/faucets/base-ethereum-sepolia-faucet)
- **OP Stack Documentation**: [https://docs.optimism.io/](https://docs.optimism.io/)
- **Ethereum JSON-RPC Specification**: [https://ethereum.org/en/developers/docs/apis/json-rpc/](https://ethereum.org/en/developers/docs/apis/json-rpc/)

---

**Need help?** Open an issue on GitHub or ask in the project Discord/Slack channel.
