# Epic 8: EVM Payment Channels (Base L2)

**Goal:** Implement production-ready XRP-style payment channel smart contracts for EVM chains (deployed to Base L2) that enable instant cryptocurrency micropayments between ILP connector peers supporting any ERC20 token. Build the complete payment channel infrastructure including EVM smart contracts (TokenNetworkRegistry and TokenNetwork following Raiden architecture), off-chain channel state management SDK, integration with Epic 6's TigerBeetle accounting layer for automatic settlement, and dashboard visualization of active payment channels. This epic delivers real cryptocurrency settlement via payment channels, replacing Epic 6's mock settlement API with on-chain settlement finality on Base L2 mainnet.

**Foundation:** This epic builds on the comprehensive payment channels research documented in `docs/research/payment-channels-research-report.md`, following the Raiden Network's proven TokenNetwork pattern with security best practices from Connext and Celer audits.

**Important:** This epic focuses on **smart contract development** for XRP-style payment channels that support **any ERC20 token** on EVM-compatible chains. The contracts will be deployed to Base L2 mainnet (a public blockchain), but connectors will simply connect to Base L2's public RPC endpoints - **no blockchain node deployment is required**. The smart contracts are fully EVM-compatible and can be deployed to any EVM chain (Ethereum, Optimism, Arbitrum, Polygon, etc.).

---

## Story 8.1: Smart Contract Development Environment Setup

As a smart contract developer,
I want a Foundry development environment for building and testing payment channel contracts,
so that I can develop XRP-style payment channels for EVM chains.

**Prerequisites:** Epic 7 "Local Blockchain Development Infrastructure" completed - Anvil running via `docker-compose-dev.yml`

### Acceptance Criteria

1. `packages/contracts/` directory created with Foundry project initialized (`forge init`)
2. Foundry configured to use local Anvil (from dev infrastructure epic) and Base L2 testnet in `foundry.toml`
3. Development wallet/private key configured using Anvil's pre-funded accounts
4. OpenZeppelin contracts library added as dependency (`forge install OpenZeppelin/openzeppelin-contracts`)
5. Deployment scripts created in `packages/contracts/script/Deploy.s.sol` for local, testnet, and mainnet
6. Environment variables configured: `BASE_RPC_URL=http://localhost:8545` (local Anvil), `BASE_SEPOLIA_RPC_URL`, `BASE_MAINNET_RPC_URL`, `PRIVATE_KEY`, `ETHERSCAN_API_KEY`
7. Documentation added to `docs/guides/smart-contract-development.md` explaining Foundry workflow with local Anvil
8. README updated with contract deployment instructions (local → testnet → mainnet progression)
9. Test deployment to local Anvil (from dev infrastructure) verifies setup is working
10. Integration test deploys contract to Anvil and verifies via connector

### Technical Notes

**Development Stack:**

- **Foundry:** Smart contract development framework (forge, cast, anvil)
- **Local Anvil:** Provided by "Local Blockchain Development Infrastructure" epic at http://localhost:8545
- **Base Sepolia:** Testnet for testing ($0 gas, faucet available)
- **Base Mainnet:** Production deployment (low gas costs ~$0.001-0.01)

**Development Workflow:**

```
1. Develop locally: Deploy to Anvil (instant, free, offline)
   ↓
2. Test on testnet: Deploy to Base Sepolia (public testnet)
   ↓
3. Security audit: Professional audit before production
   ↓
4. Deploy to mainnet: Base L2 mainnet (production)
```

**Configuration:**

```toml
# foundry.toml
[profile.default]
src = "src"
out = "out"
libs = ["lib"]
solc_version = "0.8.20"

[rpc_endpoints]
local = "http://localhost:8545"  # Local Anvil (from dev infrastructure)
base_sepolia = "${BASE_SEPOLIA_RPC_URL}"  # Public testnet
base_mainnet = "${BASE_MAINNET_RPC_URL}"  # Public mainnet

[etherscan]
base_sepolia = { key = "${ETHERSCAN_API_KEY}", url = "https://api-sepolia.basescan.org/api" }
base_mainnet = { key = "${ETHERSCAN_API_KEY}", url = "https://api.basescan.org/api" }
```

**Deployment Targets:**

- **Local Anvil:** `http://localhost:8545` (from dev infrastructure epic, instant testing)
- **Base Sepolia testnet:** `https://sepolia.base.org` (public testnet for integration testing)
- **Base Mainnet:** `https://mainnet.base.org` (production)
- **Or paid RPC providers:** Alchemy, Infura, QuickNode for better reliability

---

## Story 8.2: Smart Contract Development - TokenNetworkRegistry

As a smart contract developer,
I want a TokenNetworkRegistry factory contract that creates isolated TokenNetwork contracts per ERC20 token,
so that payment channels can support multiple tokens with security isolation.

### Acceptance Criteria

1. `TokenNetworkRegistry.sol` implemented in new `packages/contracts/src/` directory
2. Foundry project initialized in `packages/contracts/` with `forge init`
3. Registry implements `createTokenNetwork(address token)` function that deploys new TokenNetwork
4. Registry prevents duplicate TokenNetwork creation for same token (reverts if already exists)
5. Registry maintains `token_to_token_networks` mapping for lookups
6. Registry emits `TokenNetworkCreated(address indexed token, address indexed tokenNetwork)` event
7. Registry implements `getTokenNetwork(address token)` view function
8. OpenZeppelin `Ownable` inherited for admin control (pause registry if needed)
9. Solidity version 0.8.x with custom errors for gas optimization
10. Unit tests verify registry creation, duplicate prevention, and lookup functionality

### Technical Specification

**Contract Interface:**

```solidity
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "./TokenNetwork.sol";
import "@openzeppelin/contracts/access/Ownable.sol";

contract TokenNetworkRegistry is Ownable {
    mapping(address => address) public token_to_token_networks;
    mapping(address => address) public token_network_to_token;

    event TokenNetworkCreated(
        address indexed token,
        address indexed tokenNetwork
    );

    error TokenNetworkAlreadyExists(address token);
    error InvalidTokenAddress();

    function createTokenNetwork(address token) external returns (address);
    function getTokenNetwork(address token) external view returns (address);
}
```

**Security Considerations:**

- Validate token is not zero address
- Ensure token is valid ERC20 (check for `totalSupply()` function)
- Emit events for all state changes

---

## Story 8.3: Smart Contract Development - TokenNetwork Core

As a smart contract developer,
I want a TokenNetwork contract that manages payment channel lifecycle (open, deposit, close, settle),
so that two parties can establish channels and exchange off-chain signed balance proofs.

### Acceptance Criteria

1. `TokenNetwork.sol` implemented with channel state management
2. Channel identifier computed as `keccak256(abi.encodePacked(participant1, participant2, channelCounter))`
3. `openChannel(address participant2, uint256 settlementTimeout)` function creates new channel
4. `setTotalDeposit(bytes32 channelId, address participant, uint256 totalDeposit)` adds funds to channel
5. Channel state tracks: participants, deposits, withdrawn amounts, nonces, balance hashes, channel status
6. Channel status enum: `NonExistent`, `Opened`, `Closed`, `Settled`
7. OpenZeppelin `SafeERC20` used for all token transfers to handle non-standard ERC20 tokens
8. OpenZeppelin `ReentrancyGuard` applied to all external state-changing functions
9. Events emitted for all channel lifecycle transitions
10. Unit tests verify channel opening, deposits, and state transitions

### Channel State Structure

```solidity
struct Channel {
    uint256 settlementTimeout;
    ChannelState state;
    uint256 closedAt;
    mapping(address => ParticipantState) participants;
}

struct ParticipantState {
    uint256 deposit;          // Total deposited by participant
    uint256 withdrawnAmount;  // Withdrawn during channel lifetime
    bool isCloser;            // True if this participant initiated close
    uint256 nonce;            // Monotonically increasing state counter
    bytes32 balanceHash;      // Hash of transferred/locked amounts
}

enum ChannelState { NonExistent, Opened, Closed, Settled }
```

### Security Requirements

1. **Reentrancy Protection:** All functions with token transfers must be nonReentrant
2. **SafeERC20:** Use `safeTransferFrom` and `safeTransfer` for all ERC20 operations
3. **Balance Verification:** Measure actual balance changes before/after transfers (handles fee-on-transfer tokens)
4. **State Validation:** All state transitions validated (e.g., can't close already closed channel)

---

## Story 8.4: Smart Contract Development - Channel Closure and Settlement

As a smart contract developer,
I want channel closure with challenge periods and dispute resolution,
so that participants can exit channels unilaterally with protection against stale state submission.

### Acceptance Criteria

1. `closeChannel(bytes32 channelId, BalanceProof memory balanceProof, bytes memory signature)` function implemented
2. Close function validates balance proof signature using EIP-712 typed structured data
3. Close function records closing participant, balance proof, and current block timestamp
4. Challenge period starts on channel close (duration = `settlementTimeout` from channel creation)
5. `updateNonClosingBalanceProof()` allows counterparty to submit newer state during challenge
6. Newer state validated by comparing nonces (must be strictly greater)
7. `settleChannel(bytes32 channelId)` function distributes final balances after challenge period
8. Settlement calculates final balances: `deposit - transferred_to_counterparty`
9. Settlement transfers tokens to both participants using SafeERC20
10. Unit tests verify cooperative close, unilateral close, challenge submission, and final settlement

### Balance Proof Verification

**EIP-712 Signature Scheme:**

```solidity
struct BalanceProof {
    bytes32 channelId;
    uint256 nonce;
    uint256 transferredAmount;  // Cumulative amount sent to counterparty
    uint256 lockedAmount;       // Amount in pending conditional transfers
    bytes32 locksRoot;          // Merkle root of hash-locked transfers
}

bytes32 constant BALANCE_PROOF_TYPEHASH = keccak256(
    "BalanceProof(bytes32 channelId,uint256 nonce,uint256 transferredAmount,uint256 lockedAmount,bytes32 locksRoot)"
);

function verifyBalanceProof(
    BalanceProof memory proof,
    bytes memory signature,
    address signer
) internal view returns (bool) {
    bytes32 structHash = keccak256(abi.encode(
        BALANCE_PROOF_TYPEHASH,
        proof.channelId,
        proof.nonce,
        proof.transferredAmount,
        proof.lockedAmount,
        proof.locksRoot
    ));

    bytes32 digest = keccak256(abi.encodePacked(
        "\x19\x01",
        DOMAIN_SEPARATOR,
        structHash
    ));

    address recovered = ECDSA.recover(digest, signature);
    return recovered == signer && recovered != address(0);
}
```

### Security Considerations

1. **Signature Replay Prevention:** Include `channelId` and `chainId` in domain separator
2. **Stale State Protection:** Monotonic nonces prevent replaying old balance proofs
3. **Challenge Window:** Configurable settlement timeout (default: 1 hour for dev, 24 hours for production)
4. **Address Zero Check:** Always validate `ECDSA.recover` doesn't return `address(0)`

---

## Story 8.5: Smart Contract Security Hardening and Edge Cases

As a smart contract security engineer,
I want comprehensive security protections and edge case handling,
so that payment channels are safe for production use with real cryptocurrency.

### Acceptance Criteria

1. Pausable circuit breaker implemented (owner can pause all channel operations in emergency)
2. Token whitelist capability added (optional: restrict to approved ERC20 tokens only)
3. Non-standard ERC20 token handling: measure actual balance changes, not transferred amounts
4. Maximum deposit limits per channel to prevent griefing attacks (configurable, default: 1M tokens)
5. Minimum settlement timeout enforced (e.g., 1 hour minimum to prevent instant-close griefing)
6. Channel expiry mechanism: channels can be force-closed after max lifetime (e.g., 1 year)
7. Cooperative settlement function bypasses challenge period for mutual consent
8. Withdrawal function allows removing funds while channel is still open (with counterparty signature)
9. Emergency token recovery for owner (only if channel stuck in invalid state)
10. Fuzz testing suite validates all edge cases: zero amounts, maximum uint256 values, simultaneous operations

### Edge Cases Covered

**1. Fee-on-Transfer Tokens:**

```solidity
uint256 balanceBefore = token.balanceOf(address(this));
token.safeTransferFrom(participant, address(this), amount);
uint256 balanceAfter = token.balanceOf(address(this));
uint256 actualReceived = balanceAfter - balanceBefore;
// Use actualReceived, not amount
```

**2. Reentrancy via Malicious ERC20:**

- All state changes BEFORE external calls (checks-effects-interactions)
- ReentrancyGuard on all public/external functions with token transfers

**3. Griefing via Locked Channels:**

- Force close after max channel lifetime
- Penalty for submitting fraudulent balance proofs (slash deposit)

**4. Signature Malleability:**

- Use OpenZeppelin ECDSA library (handles malleability)
- Validate `v`, `r`, `s` parameters

---

## Story 8.6: Smart Contract Testing and Security Audit

As a smart contract developer,
I want comprehensive testing and professional security audit,
so that payment channels are safe for production deployment with real funds.

### Acceptance Criteria

1. Foundry test suite with >95% code coverage for all contracts
2. Unit tests cover all functions: channel open, deposit, close, settle, dispute
3. Integration tests verify multi-channel scenarios: concurrent channels, multiple tokens
4. Fuzz tests validate edge cases: random amounts, random nonces, malformed signatures
5. Gas optimization benchmarks: channel open <150k gas, close <100k gas, settle <80k gas
6. Invariant tests verify: total deposits = total withdrawals + locked amounts
7. Professional security audit contracted (OpenZeppelin, Trail of Bits, or Consensys Diligence)
8. Audit findings documented and addressed with mitigation commits
9. Testnet deployment on Base Sepolia with bug bounty program ($5k-$25k rewards)
10. Mainnet deployment plan documented with upgrade strategy and emergency procedures

### Testing Strategy

**Unit Tests (Foundry):**

```solidity
// packages/contracts/test/TokenNetwork.t.sol
contract TokenNetworkTest is Test {
    TokenNetwork network;
    MockERC20 token;

    function testOpenChannel() public { /* ... */ }
    function testDeposit() public { /* ... */ }
    function testCloseChannel() public { /* ... */ }
    function testSettleChannel() public { /* ... */ }
    function testChallenge() public { /* ... */ }
}
```

**Fuzz Tests:**

```solidity
function testFuzz_Deposit(uint256 amount) public {
    vm.assume(amount > 0 && amount < type(uint128).max);
    // Test random deposit amounts
}
```

**Invariant Tests:**

```solidity
function invariant_TotalBalanceConserved() public {
    uint256 totalDeposits = getTotalDeposits();
    uint256 totalWithdrawals = getTotalWithdrawals();
    uint256 contractBalance = token.balanceOf(address(network));
    assertEq(totalDeposits, totalWithdrawals + contractBalance);
}
```

### Audit Requirements

**Audit Scope:**

- TokenNetworkRegistry.sol
- TokenNetwork.sol
- All library contracts (ChannelLibrary, SignatureValidator, BalanceProof)
- Off-chain signature generation (TypeScript SDK)

**Audit Focus Areas:**

1. Signature verification correctness
2. Reentrancy attack vectors
3. Integer overflow/underflow (even with Solidity 0.8.x)
4. Access control and authorization
5. Funds custody and withdrawal logic
6. Challenge mechanism and dispute resolution
7. Gas optimization and DoS attack vectors

**Timeline:**

- Audit engagement: 4-6 weeks
- Remediation: 2-3 weeks
- Re-audit: 1-2 weeks

---

## Story 8.7: Off-Chain Payment Channel SDK (TypeScript)

As a connector developer,
I want a TypeScript SDK for off-chain payment channel operations,
so that I can open channels, sign balance proofs, and submit settlement transactions programmatically.

### Acceptance Criteria

1. `PaymentChannelSDK` class implemented in `packages/connector/src/settlement/payment-channel-sdk.ts`
2. SDK wraps ethers.js for Base L2 blockchain interactions
3. SDK implements `openChannel(peerAddress, settlementTimeout, initialDeposit)` method
4. SDK implements `signBalanceProof(channelId, nonce, transferredAmount)` using EIP-712
5. SDK implements `closeChannel(channelId, finalBalanceProof)` method
6. SDK implements `settleChannel(channelId)` after challenge period expires
7. SDK maintains local channel state cache (channel IDs, nonces, balances)
8. SDK exposes `getChannelBalance(channelId)` method querying on-chain contract state
9. SDK implements event listeners for on-chain channel events (ChannelOpened, ChannelClosed, ChannelSettled)
10. Unit tests verify SDK methods using local Base node and deployed test contracts

### SDK Interface

```typescript
// packages/connector/src/settlement/payment-channel-sdk.ts

interface ChannelState {
  channelId: string;
  participants: [string, string];
  myDeposit: bigint;
  theirDeposit: bigint;
  myNonce: number;
  theirNonce: number;
  myTransferred: bigint;
  theirTransferred: bigint;
  status: 'opened' | 'closed' | 'settled';
}

class PaymentChannelSDK {
  constructor(provider: ethers.Provider, signer: ethers.Signer, registryAddress: string);

  // Channel management
  async openChannel(
    participant2: string,
    tokenAddress: string,
    settlementTimeout: number,
    initialDeposit: bigint
  ): Promise<string>; // Returns channelId

  async deposit(channelId: string, amount: bigint): Promise<void>;

  // Off-chain operations
  signBalanceProof(channelId: string, nonce: number, transferredAmount: bigint): Promise<string>; // Returns signature

  verifyBalanceProof(
    channelId: string,
    nonce: number,
    transferredAmount: bigint,
    signature: string,
    signer: string
  ): Promise<boolean>;

  // On-chain settlement
  async closeChannel(
    channelId: string,
    balanceProof: BalanceProof,
    signature: string
  ): Promise<void>;

  async settleChannel(channelId: string): Promise<void>;

  // State queries
  async getChannelState(channelId: string): Promise<ChannelState>;
  async getMyChannels(): Promise<string[]>;
}
```

### EIP-712 Signing

```typescript
// EIP-712 domain separator
const domain = {
  name: 'PaymentChannel',
  version: '1',
  chainId: await provider.getNetwork().then((n) => n.chainId),
  verifyingContract: tokenNetworkAddress,
};

// EIP-712 types
const types = {
  BalanceProof: [
    { name: 'channelId', type: 'bytes32' },
    { name: 'nonce', type: 'uint256' },
    { name: 'transferredAmount', type: 'uint256' },
    { name: 'lockedAmount', type: 'uint256' },
    { name: 'locksRoot', type: 'bytes32' },
  ],
};

// Sign balance proof
const signature = await signer.signTypedData(domain, types, balanceProof);
```

---

## Story 8.8: Settlement Engine Integration with Payment Channels

As a settlement monitor,
I want to automatically execute payment channel settlements when TigerBeetle balances exceed thresholds,
so that outstanding balances are settled on-chain via Base L2 payment channels.

### Acceptance Criteria

1. `SettlementExecutor` class implemented in `packages/connector/src/settlement/settlement-executor.ts`
2. Settlement executor listens to `SETTLEMENT_REQUIRED` events from Epic 6's SettlementMonitor
3. When settlement triggered, executor checks if payment channel exists for peer
4. If channel doesn't exist, executor opens new channel with initial deposit from TigerBeetle balance
5. If channel exists, executor generates latest balance proof and submits to chain (cooperative settle)
6. Settlement executor updates TigerBeetle accounts after on-chain settlement completes
7. Settlement executor handles settlement failures (insufficient gas, channel disputes) with retry logic
8. Settlement executor emits telemetry events for settlement execution (success/failure/pending)
9. Unit tests verify settlement execution flow using mock payment channel contracts
10. Integration test verifies end-to-end flow: packets → balance exceeds threshold → channel settlement → TigerBeetle updated

### Settlement Execution Flow

```typescript
// packages/connector/src/settlement/settlement-executor.ts

class SettlementExecutor {
  constructor(
    private accountManager: AccountManager,
    private paymentChannelSDK: PaymentChannelSDK,
    private settlementMonitor: SettlementMonitor
  ) {
    // Listen for settlement triggers
    settlementMonitor.on('SETTLEMENT_REQUIRED', this.handleSettlement.bind(this));
  }

  private async handleSettlement(event: SettlementRequiredEvent) {
    const { peerId, balance, tokenAddress } = event;

    // Check if channel exists
    const channelId = await this.findChannelForPeer(peerId, tokenAddress);

    if (!channelId) {
      // Open new channel
      await this.openChannelAndSettle(peerId, tokenAddress, balance);
    } else {
      // Use existing channel
      await this.settleViaExistingChannel(channelId, balance);
    }

    // Update TigerBeetle after settlement
    await this.accountManager.recordSettlement(peerId, balance);
  }

  private async settleViaExistingChannel(channelId: string, amount: bigint) {
    // Generate latest balance proof
    const channelState = await this.paymentChannelSDK.getChannelState(channelId);
    const newNonce = channelState.myNonce + 1;
    const newTransferred = channelState.myTransferred + amount;

    // Sign and submit
    const signature = await this.paymentChannelSDK.signBalanceProof(
      channelId,
      newNonce,
      newTransferred
    );

    // Cooperative settlement (both parties agree)
    await this.paymentChannelSDK.cooperativeSettle(channelId, signature);
  }
}
```

### Integration with Epic 6

**Epic 6 provides:**

- TigerBeetle account balances
- Settlement threshold detection
- `SETTLEMENT_REQUIRED` event triggers

**Epic 8 adds:**

- Payment channel smart contracts
- On-chain settlement execution
- Blockchain transaction handling

**Combined flow:**

1. Packets flow through connector (Epic 1-2)
2. TigerBeetle records balances (Epic 6)
3. Threshold exceeded triggers settlement (Epic 6)
4. Settlement executor opens/uses payment channel (Epic 8)
5. On-chain settlement finalizes (Epic 8)
6. TigerBeetle updated with settled amounts (Epic 6)

---

## Story 8.9: Automated Channel Lifecycle Management

As a connector operator,
I want automatic payment channel lifecycle management (open when needed, close when idle),
so that channels are efficiently managed without manual intervention.

### Acceptance Criteria

1. `ChannelManager` class implemented in `packages/connector/src/settlement/channel-manager.ts`
2. Channel manager tracks all active channels per peer and token
3. Channel manager automatically opens channel when first settlement needed for peer
4. Channel manager configures initial deposit based on expected settlement frequency (default: 10x threshold)
5. Channel manager monitors channel deposit levels and adds funds when running low
6. Channel manager detects idle channels (no settlement for X hours, configurable, default: 24 hours)
7. Channel manager automatically closes idle channels cooperatively to reclaim deposits
8. Channel manager handles disputed closures (counterparty non-responsive) with unilateral close
9. Unit tests verify channel opening, deposit management, and closure logic
10. Integration test verifies channel lifecycle across multiple peers and tokens

### Channel Lifecycle State Machine

```
┌─────────────┐
│ No Channel  │
└──────┬──────┘
       │ Settlement needed
       ▼
┌─────────────┐
│   Opening   │ ──→ openChannel() transaction
└──────┬──────┘
       │ Channel opened event
       ▼
┌─────────────┐
│   Active    │ ◄──→ signBalanceProof() off-chain
│             │      (cooperative settlement)
└──────┬──────┘
       │ Idle detected (24h no activity)
       ▼
┌─────────────┐
│   Closing   │ ──→ cooperativeSettle() or closeChannel()
└──────┬──────┘
       │ Settlement period elapsed
       ▼
┌─────────────┐
│  Settling   │ ──→ settleChannel() transaction
└──────┬──────┘
       │ Channel settled event
       ▼
┌─────────────┐
│   Settled   │
└─────────────┘
```

### Configuration

```yaml
# Connector configuration
paymentChannels:
  base:
    enabled: true
    rpcUrl: http://base-node:8545
    registryAddress: '0x...'
    defaultSettlementTimeout: 86400 # 24 hours
    initialDepositMultiplier: 10 # 10x settlement threshold
    idleChannelThreshold: 86400 # Close after 24h idle
    minDepositThreshold: 0.5 # Add funds when below 50% of initial
```

---

## Story 8.10: Dashboard Payment Channel Visualization

As a dashboard user,
I want to see active payment channels in the network visualization with real-time balance updates,
so that I can monitor channel states, deposits, and settlement activity.

### Acceptance Criteria

1. `PAYMENT_CHANNEL_OPENED` telemetry event added to shared types
2. `PAYMENT_CHANNEL_SETTLED` telemetry event added to shared types
3. Channel telemetry includes: channel ID, participants, token, deposits, current balances, status
4. Dashboard backend stores channel state in memory for all active channels
5. Dashboard frontend displays channel indicator on peer connections in network graph
6. Channel indicator shows: channel status (green=active, yellow=settling, red=disputed)
7. Channel tooltip shows: deposits, transferred amounts, nonces, settlement timeout
8. Dashboard timeline view shows channel lifecycle events (opened, settled, closed)
9. Dashboard includes "Payment Channels" panel listing all channels with filter by peer/token
10. Integration test verifies channel events flow from connector to dashboard UI

### Telemetry Schema

```typescript
// packages/shared/src/types/telemetry.ts

interface PaymentChannelOpenedEvent {
  type: 'PAYMENT_CHANNEL_OPENED';
  timestamp: number;
  nodeId: string;
  channelId: string;
  participants: [string, string];
  tokenAddress: string;
  tokenSymbol: string;
  settlementTimeout: number;
  initialDeposits: {
    [participant: string]: string; // bigint as string
  };
}

interface PaymentChannelBalanceUpdate {
  type: 'PAYMENT_CHANNEL_BALANCE_UPDATE';
  timestamp: number;
  nodeId: string;
  channelId: string;
  myNonce: number;
  theirNonce: number;
  myTransferred: string; // bigint as string
  theirTransferred: string;
}

interface PaymentChannelSettledEvent {
  type: 'PAYMENT_CHANNEL_SETTLED';
  timestamp: number;
  nodeId: string;
  channelId: string;
  finalBalances: {
    [participant: string]: string;
  };
  settlementType: 'cooperative' | 'unilateral' | 'disputed';
}
```

### Dashboard UI Components

**Network Graph Enhancement:**

- Channel badge on peer edges (🔗 icon with status color)
- Hover shows channel details

**Payment Channels Panel:**

```
┌─ Payment Channels ────────────────────────┐
│ Peer: Alice | Token: USDC | Status: Active│
│ Channel ID: 0xabc...                      │
│ My Deposit: 1000 USDC                     │
│ Transferred: 250 USDC (Nonce: 42)         │
│ Settlement Timeout: 23.5 hours remaining  │
├───────────────────────────────────────────┤
│ Peer: Bob | Token: DAI | Status: Settling│
│ Channel ID: 0xdef...                      │
│ ...                                        │
└───────────────────────────────────────────┘
```

---

## Epic Completion Criteria

- [ ] TokenNetworkRegistry and TokenNetwork smart contracts deployed to Base Sepolia testnet
- [ ] Smart contracts support any ERC20 token (multi-token architecture)
- [ ] Smart contracts pass comprehensive test suite (>95% coverage)
- [ ] Professional security audit completed with findings remediated
- [ ] Payment channel SDK functional with EIP-712 signature generation
- [ ] Settlement executor automatically triggers on-chain settlement from TigerBeetle thresholds
- [ ] Channel lifecycle manager opens/closes channels automatically
- [ ] Dashboard displays active payment channels with real-time balance updates
- [ ] Integration tests verify end-to-end flow: packet → balance → threshold → channel settlement
- [ ] Documentation complete for payment channel deployment and configuration
- [ ] Contracts deployed to Base mainnet and verified on Basescan

---

## Dependencies and Integration Points

**Depends On:**

- **Epic 7: Local Blockchain Development Infrastructure** - Anvil and rippled for local testing (REQUIRED for development)
- Epic 6: TigerBeetle accounting and settlement thresholds
- Epic 2: BTP protocol for peer connections
- Epic 3: Dashboard telemetry infrastructure

**Integrates With:**

- `AccountManager` (Epic 6) - TigerBeetle balance tracking
- `SettlementMonitor` (Epic 6) - Settlement trigger events
- `PaymentChannelSDK` (Epic 8) - Blockchain interactions
- `TelemetryEmitter` - Channel event reporting

**Enables:**

- Epic 9: XRP payment channels (dual-settlement)
- Epic 10: Multi-chain settlement coordination

---

## Technical Architecture Notes

### Payment Channel vs. TigerBeetle Balance

**TigerBeetle (Off-chain accounting):**

- Tracks every ILP packet transfer instantly
- High-frequency updates (1000s per second)
- No transaction fees
- No blockchain finality delay

**Payment Channels (On-chain settlement):**

- Settles accumulated balances periodically
- Low-frequency updates (hourly/daily)
- Gas costs for settlement transactions
- Blockchain finality (seconds on Base L2)

**Flow:**

```
ILP Packets → TigerBeetle Balance → Threshold Trigger → Payment Channel Settlement
(milliseconds)  (instant)            (30s polling)       (2-3 seconds on Base)
```

### Why Deploy to Base L2?

**Base L2 is the target deployment chain for production payment channels:**

1. **Low Gas Costs:** Base L2 transactions cost $0.001-0.01 (vs. $1-50 on Ethereum mainnet)
2. **Fast Finality:** 2-second block times
3. **Ethereum Compatibility:** Full EVM support - contracts work on any EVM chain
4. **Coinbase Backing:** Production-ready RPC infrastructure, no node hosting needed
5. **Growing Ecosystem:** AI agent platforms building on Base

**But smart contracts are EVM-compatible and can deploy to:**

- Ethereum mainnet
- Optimism
- Arbitrum
- Polygon
- Any EVM-compatible chain

**Connectors just need:**

- RPC endpoint URL (e.g., `https://mainnet.base.org`)
- No local blockchain node required
- Can use public endpoints or paid providers (Alchemy, Infura)

---

## Testing Strategy

### Smart Contract Testing (Foundry)

**Unit Tests:**

- Channel lifecycle (open, deposit, close, settle)
- Signature verification (EIP-712)
- Balance calculations
- Challenge mechanism

**Fuzz Tests:**

- Random deposit amounts
- Random nonce sequences
- Malformed signatures
- Concurrent operations

**Invariant Tests:**

- Total deposits = withdrawals + locked
- Nonces always increasing
- Balance proofs always valid

**Gas Benchmarks:**

- openChannel: <150k gas
- deposit: <80k gas
- closeChannel: <100k gas
- settleChannel: <80k gas

### Integration Testing

**Scenario 1: Happy Path Settlement**

1. Connector A forwards 100 packets to Connector B
2. TigerBeetle balance reaches threshold (1000 units)
3. Settlement monitor triggers settlement
4. Channel executor opens channel and deposits
5. Balance proof signed and submitted
6. On-chain settlement completes
7. TigerBeetle balance reset to zero

**Scenario 2: Disputed Closure**

1. Channel opened between A and B
2. A initiates unilateral close with stale state
3. B submits newer balance proof during challenge
4. Settlement uses B's newer state
5. Final balances distributed correctly

**Scenario 3: Multi-Token Settlement**

1. Channels opened for USDC, DAI, USDT
2. Packets flow for all three tokens
3. All tokens reach settlement thresholds
4. Three separate channel settlements execute
5. All TigerBeetle balances updated

---

## Security Considerations

### Smart Contract Security

1. **Reentrancy Protection:** ReentrancyGuard on all token transfers
2. **SafeERC20:** Handle non-standard tokens safely
3. **Signature Verification:** EIP-712 with OpenZeppelin ECDSA
4. **Access Control:** Ownable for admin functions
5. **Pausable:** Emergency circuit breaker
6. **Challenge Period:** Protects against stale state submission

### Off-Chain Security

1. **Private Key Management:** Connector private keys stored in environment variables (Epic 9: HSM/KMS)
2. **Nonce Management:** Prevent nonce reuse and replay attacks
3. **Balance Proof Storage:** Persist signed proofs for dispute resolution
4. **Gas Management:** Monitor gas prices and set appropriate limits
5. **Rate Limiting:** Prevent excessive on-chain transactions

### Audit Focus Areas

1. Signature verification correctness (EIP-712)
2. Reentrancy attack vectors
3. Integer overflow/underflow
4. Funds custody and withdrawal
5. Challenge mechanism correctness
6. Gas optimization and DoS vectors
7. Non-standard ERC20 handling

---

## Performance Requirements

- **Settlement Latency:** <5 seconds from trigger to on-chain confirmation
- **Channel Opening:** <10 seconds including block confirmations
- **Gas Efficiency:** <$0.01 per settlement on Base L2
- **Concurrent Channels:** Support 100+ active channels per connector
- **Throughput:** Handle 10 settlements/minute without bottleneck

---

## Documentation Deliverables

1. `docs/guides/smart-contract-development.md` - Foundry setup and development workflow
2. `docs/guides/payment-channels-overview.md` - Payment channel concepts and usage
3. `docs/guides/smart-contract-deployment.md` - Contract deployment to Base L2 (testnet and mainnet)
4. `docs/architecture/payment-channel-architecture.md` - Technical architecture details
5. `docs/api/payment-channel-sdk.md` - TypeScript SDK API reference
6. Smart contract NatSpec documentation (auto-generated)
7. Security audit report (public after remediation)
8. `docs/guides/base-l2-rpc-configuration.md` - Connecting to Base L2 RPC endpoints (public and paid providers)

---

## Success Metrics

- Smart contract deployment success rate: 100%
- Settlement execution success rate: >99%
- Average settlement latency: <5 seconds
- Gas cost per settlement: <$0.01 on Base L2
- Channel uptime: >99.9% (no stuck channels)
- Dashboard channel visualization latency: <1 second from on-chain event
- Zero critical security vulnerabilities after audit

---

## Timeline Estimate

**Total Duration:** 12-17 weeks

- **Week 1:** Foundry setup and development environment (Story 8.1)
- **Weeks 2-5:** Smart contract development (Stories 8.2-8.4)
- **Weeks 6-8:** Security hardening and edge cases (Story 8.5)
- **Weeks 9-11:** Testing and audit preparation (Story 8.6)
- **Weeks 12-15:** Security audit and remediation (Story 8.6)
- **Weeks 13-15:** SDK development and integration (Stories 8.7-8.8) - parallel with audit
- **Weeks 16-17:** Channel lifecycle and dashboard (Stories 8.9-8.10)

**Critical Path:** Security audit (4-6 weeks) is the longest dependency

**Note:** Timeline reduced by ~1 week since no blockchain node deployment is required - connectors simply connect to Base L2's public RPC endpoints
