# XRP-Style Payment Channels as EVM Smart Contracts: Comprehensive Research Report

**Research Date:** January 2, 2026
**Objective:** Design and implementation guidance for multi-token, multi-user payment channels on EVM-compatible blockchains

---

## EXECUTIVE SUMMARY

### Recommended Technical Approach

After comprehensive research into XRP payment channels, Ethereum state channel implementations (Raiden, Connext, Celer, Perun), and EVM smart contract patterns, the following architecture is recommended for building XRP-style payment channels with multi-token and multi-user support:

**Core Architecture:**

- **Registry Pattern:** Single `TokenNetworkRegistry` contract that deploys separate `TokenNetwork` contracts for each ERC20 token
- **Channel Model:** Duplex channels with two unidirectional simplex channels for efficiency
- **Signature Scheme:** EIP-712 typed structured data signing for human-readable, secure signatures
- **Settlement Model:** Cooperative settlement preferred, unilateral settlement with challenge windows as fallback
- **Multi-Token Support:** Isolated token networks rather than single contract handling all tokens (better gas efficiency and security isolation)

**Recommended Stack:**

- Solidity 0.8.x with custom errors for gas optimization
- OpenZeppelin libraries (SafeERC20, ReentrancyGuard, ECDSA)
- Foundry for testing with built-in fuzzing
- EIP-712 for signature verification
- Pausable/circuit breaker patterns for emergency scenarios

### Top 5 Critical Security Risks and Mitigations

1. **Reentrancy Attacks via ERC20 Transfers**
   - **Risk:** Malicious ERC20 tokens can re-enter during deposit/withdrawal/settlement
   - **Mitigation:** Use OpenZeppelin's ReentrancyGuard on all external token interactions, follow checks-effects-interactions pattern, use SafeERC20 wrapper

2. **Signature Replay and Malleability**
   - **Risk:** Signatures can be replayed across chains or manipulated due to ECDSA malleability
   - **Mitigation:** Include chainId in EIP-712 domain separator, validate signature parameters (v, s ranges), use nonces/sequence numbers, check for address(0) on ecrecover

3. **Non-Standard ERC20 Token Behavior**
   - **Risk:** Fee-on-transfer, rebasing, or deflationary tokens break balance accounting
   - **Mitigation:** Measure actual balance changes before/after transfers, whitelist compatible tokens, explicitly document unsupported token types

4. **Griefing via Locked Channels**
   - **Risk:** Malicious user opens channels and never closes, locking counterparty funds
   - **Mitigation:** Implement unilateral closure with challenge windows, allow forced expiration after timeout, consider deposit penalties for griefing

5. **Stale State Submission**
   - **Risk:** User submits old channel state during closure to steal funds
   - **Mitigation:** Monotonically increasing nonces, challenge period allowing newer state submission, penalty mechanisms for fraudulent closure attempts

### Implementation Complexity Assessment

**Complexity: Medium-High**

**Breakdown:**

- **Smart Contracts:** Medium - Core logic is well-understood, but edge cases are complex
- **Security Considerations:** High - Multiple attack vectors require careful mitigation
- **Testing Requirements:** High - Requires extensive fuzzing and formal verification
- **Off-Chain Components:** Medium - Signature generation and state management are straightforward
- **Gas Optimization:** Medium - Requires careful storage layout and operation batching

**Estimated Development Timeline:**

- Core smart contracts: 3-4 weeks
- Security hardening: 2-3 weeks
- Testing and fuzzing: 2-3 weeks
- Off-chain client SDK: 2 weeks
- Security audit: 4-6 weeks
- Total: 13-18 weeks

### Recommended Next Steps

1. **Week 1-2: Architecture Design**
   - Finalize contract structure based on Raiden/Celer patterns
   - Design token whitelist strategy vs permissionless approach
   - Define channel state structure and signature format
   - Create detailed technical specification document

2. **Week 3-6: Core Implementation**
   - Implement TokenNetworkRegistry and TokenNetwork contracts
   - Build channel lifecycle functions (open, deposit, close, settle)
   - Integrate OpenZeppelin security libraries
   - Implement EIP-712 signature verification

3. **Week 7-9: Security Hardening**
   - Add reentrancy guards and SafeERC20
   - Implement challenge windows and dispute resolution
   - Add circuit breakers and emergency pause
   - Handle edge cases for non-standard tokens

4. **Week 10-12: Testing**
   - Unit tests with Foundry
   - Fuzz testing for edge cases
   - Integration tests with multiple tokens
   - Gas optimization benchmarking

5. **Week 13-18: Audit and Deployment**
   - Professional security audit (ConsenSys, OpenZeppelin, or Trail of Bits)
   - Address audit findings
   - Testnet deployment and bug bounty
   - Mainnet deployment strategy

---

## TECHNICAL ARCHITECTURE SPECIFICATION

### Smart Contract Structure

```
PaymentChannelSystem/
├── TokenNetworkRegistry.sol      // Main registry for token networks
├── TokenNetwork.sol              // Manages channels for one ERC20 token
├── libraries/
│   ├── ChannelLibrary.sol       // Channel state and operations
│   ├── SignatureValidator.sol   // EIP-712 signature verification
│   └── BalanceProof.sol         // Balance proof verification
└── interfaces/
    ├── ITokenNetwork.sol
    └── IERC20Channel.sol
```

### Core Components

#### 1. TokenNetworkRegistry

**Purpose:** Factory for creating and tracking TokenNetwork contracts

```solidity
contract TokenNetworkRegistry {
    mapping(address => address) public token_to_token_networks;
    mapping(address => address) public token_network_to_token;

    function createTokenNetwork(address token) external returns (address);
    function getTokenNetwork(address token) external view returns (address);
}
```

**Key Features:**

- One TokenNetwork per ERC20 token
- Prevents duplicate token networks
- Provides lookup functionality
- Emits events for network creation

#### 2. TokenNetwork

**Purpose:** Manages all payment channels for a specific ERC20 token

```solidity
contract TokenNetwork {
    struct Channel {
        uint256 settlementTimeout;
        ChannelState state;
        mapping(address => ParticipantState) participants;
    }

    struct ParticipantState {
        uint256 deposit;
        uint256 withdrawnAmount;
        bool isCloser;
        uint256 nonce;
        bytes32 balanceHash;
        uint256 lockedAmount;
    }

    enum ChannelState { NonExistent, Opened, Closed, Settled }

    mapping(bytes32 => Channel) public channels;
}
```

**Channel Identifier:** `keccak256(abi.encodePacked(participant1, participant2, channelCounter))`

### Channel Lifecycle

```
┌─────────────┐
│ Non-Existent│
└──────┬──────┘
       │ openChannel()
       ▼
┌─────────────┐
│   Opened    │◄─── setTotalDeposit()
└──────┬──────┘     setTotalWithdraw()
       │ closeChannel()
       │ or cooperativeSettle()
       ▼
┌─────────────┐
│   Closed    │◄─── updateNonClosingBalanceProof()
└──────┬──────┘     (during challenge period)
       │ settleChannel()
       │ (after challenge period)
       ▼
┌─────────────┐
│   Settled   │
└─────────────┘
```

### State Management

#### Channel State Structure

```solidity
struct ChannelState {
    // Static data
    address[2] participants;
    uint256 settlementTimeout;

    // Dynamic state
    ChannelStatus status;
    uint256 closedAt;

    // Per-participant state
    mapping(address => ParticipantInfo) participantInfo;
}

struct ParticipantInfo {
    uint256 deposit;
    uint256 withdrawn;
    uint256 nonce;
    bytes32 balanceHash;      // Hash of (transferredAmount, lockedAmount)
    uint256 transferredAmount; // Only stored after closure
}
```

#### Balance Proof

Off-chain signed messages exchanged between participants:

```solidity
struct BalanceProof {
    uint256 chainId;
    address tokenNetwork;
    bytes32 channelId;
    uint256 nonce;              // Monotonically increasing
    uint256 transferredAmount;  // Cumulative amount transferred
    uint256 lockedAmount;       // Amount in pending transfers
    bytes32 locksRoot;          // Merkle root of pending locks
}
```

### Multi-Token Management Approach

**Recommended: Separate TokenNetwork per Token**

**Advantages:**

- Gas efficient: No token address in storage per channel
- Security isolation: Bug in one token doesn't affect others
- Upgradeable: Can deploy new TokenNetwork for new token
- Proven pattern: Used by Raiden Network successfully

**Alternative: Single Universal Contract (Not Recommended)**

**Disadvantages:**

- Higher gas costs: Must store token address per channel
- Single point of failure: One bug affects all tokens
- Complex state management: Must track multiple token types
- Higher audit costs: More complex security surface

### Signature Schemes and Cryptographic Requirements

#### EIP-712 Typed Structured Data

**Domain Separator:**

```solidity
bytes32 public DOMAIN_SEPARATOR = keccak256(abi.encode(
    keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"),
    keccak256(bytes("PaymentChannel")),
    keccak256(bytes("1")),
    block.chainid,
    address(this)
));
```

**Balance Proof Type Hash:**

```solidity
bytes32 public constant BALANCE_PROOF_TYPEHASH = keccak256(
    "BalanceProof(bytes32 channelId,uint256 nonce,uint256 transferredAmount,uint256 lockedAmount,bytes32 locksRoot)"
);
```

**Signature Verification:**

```solidity
function verifySignature(
    BalanceProof memory proof,
    bytes memory signature,
    address signer
) public view returns (bool) {
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

    // Use OpenZeppelin ECDSA library for safe recovery
    address recovered = ECDSA.recover(digest, signature);
    return recovered == signer && recovered != address(0);
}
```

**Security Requirements:**

1. **Always use OpenZeppelin ECDSA library** - Handles signature malleability
2. **Check for address(0)** - ecrecover returns 0x0 on invalid signatures
3. **Include chainId** - Prevents cross-chain replay attacks
4. **Validate v and s parameters** - Prevents ECDSA malleability
5. **Use nonces** - Prevents replay of old signatures

---

## COMPARATIVE ANALYSIS

### Detailed Comparison Table

| Feature                  | XRP Channels              | Raiden                                | Connext                         | Perun                        | Celer Network              |
| ------------------------ | ------------------------- | ------------------------------------- | ------------------------------- | ---------------------------- | -------------------------- |
| **Blockchain**           | XRP Ledger                | Ethereum                              | Ethereum                        | Ethereum/Multi-chain         | Ethereum/Multi-chain       |
| **Channel Type**         | Unidirectional            | Bidirectional                         | Bidirectional                   | Bidirectional/Virtual        | Duplex (2 simplex)         |
| **Token Support**        | XRP only                  | One ERC20 per TokenNetwork            | ERC20 + ETH                     | ERC20 + ETH                  | ERC20 + ETH                |
| **Multi-User**           | Two parties               | Two parties per channel               | Two parties per channel         | Multi-party virtual channels | Two parties per duplex     |
| **Settlement**           | Claim-based with expiry   | Challenge period (500 blocks default) | Challenge period                | Challenge period             | Cooperative + Unilateral   |
| **Signature**            | ECDSA/Ed25519             | ECDSA (secp256k1)                     | ECDSA                           | ECDSA                        | ECDSA                      |
| **Network Support**      | No (direct channels only) | Network of channels                   | Network via routers             | Virtual channels via hubs    | Virtual channels supported |
| **Smart Contracts**      | Built-in ledger           | Solidity 0.8.x                        | Solidity 0.8.x                  | Solidity                     | Solidity 0.5.x+            |
| **Gas Cost (Open)**      | N/A                       | ~100,000-150,000                      | ~80,000-120,000                 | ~120,000-180,000             | ~100,000-140,000           |
| **Gas Cost (Close)**     | N/A                       | ~70,000-100,000                       | ~60,000-90,000                  | ~80,000-120,000              | ~70,000-110,000            |
| **Challenge Window**     | Expiry + Settlement delay | Configurable (default 500 blocks)     | Configurable                    | Configurable                 | Configurable               |
| **State Format**         | Balance + Claims          | Balance proofs with nonces            | Balance + conditional transfers | State hashes                 | Balance proofs             |
| **Conditional Payments** | Yes (via claims)          | Hash-time locks                       | Conditional transfers           | General conditions           | Hash locks + contracts     |
| **Dispute Resolution**   | On-ledger with claims     | On-chain settlement                   | On-chain with routers           | On-chain adjudicator         | On-chain with challenge    |
| **Upgradability**        | No (ledger-native)        | Proxy patterns possible               | Proxy patterns used             | Proxy patterns possible      | Modular design             |
| **Audit Status**         | Core ledger               | Multiple audits                       | Multiple audits (0xMacro, etc.) | Academic research + audits   | Multiple audits            |
| **Production Status**    | Production                | Production (limited use)              | Production                      | Research/Production          | Production                 |
| **License**              | ISC                       | MIT                                   | MIT                             | Apache 2.0                   | Apache 2.0/MIT             |

### Detailed Analysis by Implementation

#### XRP Payment Channels

**Architecture:**

- Built into XRP Ledger at protocol level
- Uses `PaymentChannelCreate` transaction to establish channel
- Sender signs claims off-chain, recipient verifies
- Claims have monotonically increasing amounts
- Settlement delay protects recipient from early closure

**Pros:**

- Extremely efficient (protocol-level)
- Simple unidirectional model
- Fast Ed25519 signature verification (70,000+ sigs/sec)
- Well-tested in production

**Cons:**

- XRP only (no multi-token)
- Cannot extend or customize
- No network routing built-in
- Unidirectional only

**EVM Adaptation Insights:**

- Claim-based model is simpler than bidirectional
- Could implement similar expiry + settlement delay
- Signature verification pattern is sound

#### Raiden Network

**Architecture:**

- `TokenNetworkRegistry` creates `TokenNetwork` per ERC20
- Channels store monotonically increasing deposits/withdrawals
- Balance proofs track transferred amounts with nonces
- Challenge period allows dispute of stale state
- Supports channel networks via hash-locked transfers

**Pros:**

- Proven production architecture
- Clean separation: one token = one contract
- Excellent documentation and specifications
- Multiple successful audits
- Supports token networks

**Cons:**

- Project development slowed (rollups preferred)
- Complex for simple use cases
- Higher gas costs than optimized implementations
- Limited recent updates

**Key Learnings:**

- Monotonic deposits/withdrawals pattern is gas-efficient
- Separate TokenNetwork per token is best approach
- Nonce-based state updates prevent replay
- Challenge windows should be configurable

**Code Reference:** https://github.com/raiden-network/raiden-contracts

#### Connext Network

**Architecture:**

- Focus on cross-chain communication via `xcall` primitive
- Vector protocol for generalized state channels
- Supports conditional transfers
- Router-based network topology

**Pros:**

- Cross-chain support
- Active development (audits in 2023-2024)
- Clean modular architecture
- Strong security practices

**Cons:**

- More complex than needed for simple channels
- Router dependency
- Cross-chain focus adds complexity

**Key Learnings:**

- Conditional transfers enable complex patterns
- Router architecture enables networks
- Multiple recent audits available for review
- Modular design supports upgrades

**Audit Reference:** https://github.com/connext/audits

#### Perun Network

**Architecture:**

- Virtual payment channels over existing channels
- `AssetHolder` manages collateral
- `Adjudicator` handles disputes
- Supports multi-party virtual channels
- Research-focused with production implementations

**Pros:**

- Virtual channels reduce on-chain operations
- Multi-party support
- Strong academic foundation
- Go-Perun library for off-chain logic

**Cons:**

- More complex setup
- Virtual channels require intermediaries
- Higher initial learning curve

**Key Learnings:**

- Virtual channels powerful for network topology
- Separation of asset holding and adjudication
- Multi-party channels possible but complex
- Academic rigor ensures correctness

**Research Paper:** https://eprint.iacr.org/2017/635.pdf

#### Celer Network

**Architecture:**

- Duplex channels = 2 unidirectional simplex channels
- Decoupled payment (CelerPay) and app (CelerApp) layers
- `CelerWallet` holds multi-owner, multi-token funds
- `PayResolver` handles conditional payment resolution
- Supports both cooperative and unilateral settlement

**Pros:**

- Duplex design simplifies off-chain logic
- Conditional payments with multiple types
- Flexible architecture
- Well-documented API

**Cons:**

- More complex contract structure
- Multiple contract interactions increase gas
- Older Solidity versions

**Key Learnings:**

- Duplex model is more intuitive than bidirectional
- Conditional payment framework is powerful
- Modular design enables feature additions
- Challenge windows essential for security

**Documentation:** https://celer.network/docs/celercore/channel/overview.html

---

## SECURITY ANALYSIS

### Complete Threat Model

#### 1. Reentrancy Attacks

**Attack Vector:**

- Malicious ERC20 token with reentrant `transfer` or `transferFrom`
- Attacker deposits malicious token, triggers reentrancy during withdrawal
- Contract state manipulated before first call completes

**Scenario:**

```solidity
// Vulnerable pattern
function withdraw(uint256 amount) external {
    require(balances[msg.sender] >= amount);
    token.transfer(msg.sender, amount);  // Reentrant call
    balances[msg.sender] -= amount;      // State update after external call
}
```

**Mitigation Strategies:**

1. **OpenZeppelin ReentrancyGuard:** Add `nonReentrant` modifier to all external functions
2. **Checks-Effects-Interactions Pattern:** Update state before external calls
3. **SafeERC20 Wrapper:** Use OpenZeppelin's SafeERC20 for all token operations
4. **Pull Payment Pattern:** Let users withdraw rather than pushing funds

**Implementation:**

```solidity
import "@openzeppelin/contracts/security/ReentrancyGuard.sol";
import "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";

contract TokenNetwork is ReentrancyGuard {
    using SafeERC20 for IERC20;

    function withdraw(uint256 amount) external nonReentrant {
        require(balances[msg.sender] >= amount, "Insufficient balance");
        balances[msg.sender] -= amount;  // Effect
        token.safeTransfer(msg.sender, amount);  // Interaction
    }
}
```

#### 2. Signature Replay Attacks

**Attack Vectors:**

**A. Cross-Chain Replay:**

- Signature valid on mainnet replayed on testnet or L2
- Same addresses on different chains enable replay

**B. Cross-Channel Replay:**

- Signature from one channel replayed in another channel
- If channel ID not properly included in signature

**C. Historical Replay:**

- Old signature replayed after channel state updated
- Without nonce validation, old state can be submitted

**Mitigation Strategies:**

1. **Include chainId in EIP-712 Domain Separator:**

```solidity
bytes32 DOMAIN_SEPARATOR = keccak256(abi.encode(
    DOMAIN_TYPEHASH,
    keccak256("PaymentChannel"),
    keccak256("1"),
    block.chainid,  // Prevents cross-chain replay
    address(this)
));
```

2. **Include Channel ID in Signed Message:**

```solidity
bytes32 structHash = keccak256(abi.encode(
    BALANCE_PROOF_TYPEHASH,
    channelId,  // Unique per channel
    nonce,
    transferredAmount,
    // ...
));
```

3. **Monotonically Increasing Nonces:**

```solidity
require(proof.nonce > channels[channelId].participants[signer].nonce, "Nonce not higher");
channels[channelId].participants[signer].nonce = proof.nonce;
```

4. **Check for address(0) on ecrecover:**

```solidity
address recovered = ECDSA.recover(digest, signature);
require(recovered != address(0) && recovered == expectedSigner, "Invalid signature");
```

#### 3. Signature Malleability

**Attack Vector:**

- ECDSA signatures have malleability: for signature (r, s, v), signature (r, -s mod n, v') is also valid
- Attacker can create equivalent signature without private key
- Can bypass signature uniqueness checks

**Technical Details:**

- For secp256k1 curve order n, if (r, s) is valid, then (r, n - s) is also valid
- Only s values in lower half of curve are canonical

**Mitigation Strategies:**

1. **Use OpenZeppelin ECDSA Library:**

```solidity
import "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";

// OpenZeppelin automatically validates s is in lower half
address signer = ECDSA.recover(hash, signature);
```

2. **Manual Validation (if not using OpenZeppelin):**

```solidity
// secp256k1 curve order
uint256 constant N = 0xfffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141;

function validateSignature(bytes32 r, bytes32 s, uint8 v) internal pure {
    require(uint256(s) <= N / 2, "Invalid signature 's' value");
    require(v == 27 || v == 28, "Invalid signature 'v' value");
}
```

#### 4. Non-Standard ERC20 Tokens

**Problem Tokens:**

**A. Fee-on-Transfer Tokens (e.g., USDT with fees enabled):**

- Token deducts fee on transfer
- Contract receives less than specified amount
- Balance accounting becomes incorrect

**B. Rebasing Tokens (e.g., Aave aTokens, Ampleforth):**

- Balance changes without transfers
- Channel balances become desynchronized
- Settlement amounts incorrect

**C. Deflationary Tokens:**

- Burn percentage on each transfer
- Similar to fee-on-transfer issues

**D. Tokens without Return Values (e.g., USDT, BNB):**

- Don't return bool on transfer/transferFrom
- Standard ERC20 interface expects bool return
- Can cause transaction reversion

**Mitigation Strategies:**

1. **Measure Actual Balance Changes:**

```solidity
function deposit(uint256 amount) external {
    uint256 balanceBefore = token.balanceOf(address(this));
    token.safeTransferFrom(msg.sender, address(this), amount);
    uint256 balanceAfter = token.balanceOf(address(this));

    uint256 actualAmount = balanceAfter - balanceBefore;
    deposits[msg.sender] += actualAmount;
}
```

2. **Use SafeERC20:**

```solidity
using SafeERC20 for IERC20;

// Handles tokens without return values
token.safeTransfer(recipient, amount);
token.safeTransferFrom(sender, recipient, amount);
```

3. **Token Whitelist:**

```solidity
mapping(address => bool) public allowedTokens;

function createTokenNetwork(address token) external {
    require(allowedTokens[token], "Token not whitelisted");
    // ...
}
```

4. **Explicit Documentation:**

- Document which token types are NOT supported
- Warn users about rebasing/fee-on-transfer tokens
- Provide testing guidelines for new tokens

#### 5. Griefing Attacks

**Attack Vectors:**

**A. Channel Locking:**

- Attacker opens many channels with victims
- Never closes or responds to closure
- Victim's funds locked until forced closure

**B. Spam Channel Creation:**

- Create many channels with minimal deposits
- Bloat contract storage
- Increase gas costs for all users

**C. Challenge Window Griefing:**

- Submit closure immediately before victim needs funds
- Force victim to wait entire challenge window
- Economic denial-of-service

**Mitigation Strategies:**

1. **Unilateral Closure:**

```solidity
function closeChannel(
    bytes32 channelId,
    BalanceProof memory proof,
    bytes memory signature
) external {
    Channel storage channel = channels[channelId];
    require(channel.state == ChannelState.Opened, "Channel not open");

    // Either participant can initiate closure
    require(isParticipant(channelId, msg.sender), "Not participant");

    channel.state = ChannelState.Closed;
    channel.closedAt = block.timestamp;
    channel.closer = msg.sender;
}
```

2. **Configurable Timeouts:**

```solidity
// Shorter timeout for small amounts, longer for large
function calculateTimeout(uint256 channelBalance) internal pure returns (uint256) {
    if (channelBalance < 1 ether) return 1 days;
    if (channelBalance < 10 ether) return 3 days;
    return 7 days;
}
```

3. **Minimum Deposit Requirements:**

```solidity
uint256 public constant MIN_DEPOSIT = 0.01 ether;

function openChannel(address partner, uint256 deposit) external {
    require(deposit >= MIN_DEPOSIT, "Deposit too small");
    // ...
}
```

4. **Griefing Penalties:**

```solidity
// Slash deposit if submitting provably old state
if (closingProof.nonce < latestProof.nonce) {
    // Penalize malicious closer
    slashedAmount = channel.participants[closer].deposit / 10;  // 10% penalty
    channel.participants[counterparty].deposit += slashedAmount;
}
```

#### 6. Stale State Submission

**Attack Vector:**

- Alice and Bob have channel with 100 transfers
- Bob submits state from transfer #10 during closure
- If not challenged, Bob steals Alice's funds

**Scenario:**

```
Initial: Alice: 50, Bob: 50
After 100 transfers: Alice: 30, Bob: 70
Bob submits state from transfer #10: Alice: 45, Bob: 55
Bob gains 15 tokens if unchallenged
```

**Mitigation Strategies:**

1. **Monotonic Nonces:**

```solidity
struct ParticipantState {
    uint256 nonce;  // Must always increase
    bytes32 balanceHash;
}

function updateNonClosingBalanceProof(
    bytes32 channelId,
    BalanceProof memory proof,
    bytes memory signature
) external {
    require(proof.nonce > channel.participants[nonCloser].nonce,
            "Nonce must be higher");
    // Update with newer state
}
```

2. **Challenge Period:**

```solidity
function settleChannel(bytes32 channelId) external {
    Channel storage channel = channels[channelId];
    require(channel.state == ChannelState.Closed, "Not closed");
    require(
        block.timestamp >= channel.closedAt + channel.settlementTimeout,
        "Challenge period not ended"
    );
    // Settle with final state
}
```

3. **Penalty for Fraud:**

```solidity
function challengeClose(
    bytes32 channelId,
    BalanceProof memory newerProof,
    bytes memory signature
) external {
    require(newerProof.nonce > closingProof.nonce, "Not newer");

    // Slash malicious closer's deposit
    uint256 penalty = channel.participants[closer].deposit;
    channel.participants[challenger].reward += penalty;
}
```

#### 7. DoS via Block Gas Limit

**Attack Vector:**

- Attacker creates many pending transfers with locks
- Settlement requires processing all locks
- Gas required exceeds block limit
- Channel becomes un-settleable

**Mitigation Strategies:**

1. **Limit Pending Transfers:**

```solidity
uint256 public constant MAX_PENDING_TRANSFERS = 100;

function lockTransfer(...) external {
    require(
        channel.pendingTransfers.length < MAX_PENDING_TRANSFERS,
        "Too many pending transfers"
    );
}
```

2. **Batch Settlement:**

```solidity
function settleChannelBatch(
    bytes32 channelId,
    uint256 startIndex,
    uint256 endIndex
) external {
    // Process locks in batches
    for (uint256 i = startIndex; i < endIndex; i++) {
        // Process lock i
    }
}
```

3. **Off-Chain Lock Resolution:**

- Participants resolve locks off-chain
- Submit only final resolved state on-chain
- Reduces on-chain complexity

### Best Practices from Audited Implementations

1. **Raiden Network Audits:**
   - Always use SafeERC20 for token interactions
   - Implement comprehensive event logging
   - Use custom errors (Solidity 0.8.4+) for gas savings
   - Separate concerns: registry, token networks, channels

2. **Connext Audits (0xMacro 2023-2024):**
   - Rigorous external review for all code changes
   - Close collaboration with security community
   - Comprehensive test coverage with edge cases
   - Clear documentation of security assumptions

3. **General Findings:**
   - Circuit breakers essential for emergency response
   - Upgradeability vs immutability trade-off carefully considered
   - Gas optimization should not compromise security
   - Extensive fuzzing reveals edge cases

### Testing and Verification Approaches

#### 1. Unit Testing with Foundry

**Advantages:**

- Tests written in Solidity
- Built-in fuzzing support
- Fast execution
- Gas reporting

**Example Test Structure:**

```solidity
contract TokenNetworkTest is Test {
    TokenNetwork network;
    MockERC20 token;

    function setUp() public {
        token = new MockERC20();
        network = new TokenNetwork(address(token));
    }

    function testOpenChannel() public {
        // Test channel opening
    }

    function testFuzz_Deposit(uint256 amount) public {
        // Fuzz test deposits with random amounts
        vm.assume(amount > 0 && amount < type(uint128).max);
        // ...
    }
}
```

#### 2. Fuzz Testing

**Critical Areas for Fuzzing:**

- Deposit and withdrawal amounts
- Nonce values and ordering
- Signature components (v, r, s)
- Token amounts with fee-on-transfer
- Timestamp manipulation
- Channel state transitions

**Tools:**

- **Foundry built-in fuzzer:** Property-based testing
- **Echidna:** Grammar-based fuzzing for invariants
- **Diligence Fuzzing:** Continuous fuzzing service

**Example Invariant:**

```solidity
// Invariant: Total deposits >= Total withdrawals + Locked amounts
function invariant_totalBalance() public {
    uint256 totalDeposits = getTotalDeposits();
    uint256 totalWithdrawn = getTotalWithdrawn();
    uint256 totalLocked = getTotalLocked();

    assertGe(totalDeposits, totalWithdrawn + totalLocked);
}
```

#### 3. Integration Testing

**Test Scenarios:**

- Full channel lifecycle (open → transfer → close → settle)
- Multiple concurrent channels
- Different ERC20 tokens (including edge cases)
- Challenge and dispute flows
- Emergency pause and recovery
- Upgrade scenarios (if upgradeable)

#### 4. Formal Verification

**Tools:**

- **Certora Prover:** Formal verification of Solidity
- **K Framework:** Formal semantics and verification
- **SMTChecker:** Solidity's built-in SMT-based verification

**Properties to Verify:**

- Conservation of funds: deposits = withdrawals + balances
- State machine correctness: no invalid transitions
- Access control: only authorized actions
- Arithmetic safety: no overflow/underflow

#### 5. Audit Preparation

**Pre-Audit Checklist:**

- [ ] 100% test coverage on critical paths
- [ ] Fuzz tests for all user inputs
- [ ] Integration tests with real ERC20 tokens
- [ ] Gas optimization benchmarks
- [ ] Comprehensive documentation
- [ ] Known issues documented
- [ ] Emergency procedures defined
- [ ] Upgrade plan documented (if applicable)

**Recommended Auditors:**

- **OpenZeppelin:** Excellent for DeFi and token contracts
- **Trail of Bits:** Deep security expertise
- **ConsenSys Diligence:** Ethereum-focused, comprehensive
- **0xMacro:** Recent Connext audits, good track record
- **Certik:** Large firm with automated tools

---

## GAS OPTIMIZATION STRATEGIES

### Estimated Costs Per Operation

Based on research of Raiden, Celer, and other implementations:

| Operation                | Estimated Gas         | Cost at 50 gwei | Cost at 100 gwei |
| ------------------------ | --------------------- | --------------- | ---------------- |
| **Registry Operations**  |                       |                 |                  |
| Create TokenNetwork      | 2,000,000 - 3,000,000 | $4.00 - $6.00   | $8.00 - $12.00   |
|                          |                       |                 |                  |
| **Channel Lifecycle**    |                       |                 |                  |
| Open Channel             | 80,000 - 120,000      | $0.16 - $0.24   | $0.32 - $0.48    |
| Deposit (first time)     | 60,000 - 80,000       | $0.12 - $0.16   | $0.24 - $0.32    |
| Deposit (additional)     | 40,000 - 60,000       | $0.08 - $0.12   | $0.16 - $0.24    |
| Withdraw                 | 50,000 - 70,000       | $0.10 - $0.14   | $0.20 - $0.28    |
| Cooperative Close        | 60,000 - 90,000       | $0.12 - $0.18   | $0.24 - $0.36    |
| Unilateral Close         | 80,000 - 120,000      | $0.16 - $0.24   | $0.32 - $0.48    |
| Update Non-Closing Proof | 70,000 - 100,000      | $0.14 - $0.20   | $0.28 - $0.40    |
| Settle Channel           | 70,000 - 110,000      | $0.14 - $0.22   | $0.28 - $0.44    |
|                          |                       |                 |                  |
| **Off-Chain**            |                       |                 |                  |
| Sign Balance Proof       | 0 (off-chain)         | $0.00           | $0.00            |
| Verify Signature         | 0 (off-chain)         | $0.00           | $0.00            |
| Exchange Messages        | 0 (off-chain)         | $0.00           | $0.00            |

**Notes:**

- Gas prices vary significantly: 50 gwei is moderate, 100 gwei is high
- L2 costs are 10-100x lower (Arbitrum, Optimism)
- Off-chain operations have zero gas cost
- Batch operations can reduce per-operation costs

### Specific Optimization Techniques

#### 1. Storage Layout Optimization

**Pack Variables into 32-byte Slots:**

```solidity
// Bad: Uses 3 storage slots
struct Channel {
    address participant1;      // 20 bytes (slot 1)
    address participant2;      // 20 bytes (slot 2)
    uint256 settlementTimeout; // 32 bytes (slot 3)
    ChannelState state;        // 32 bytes (slot 4)
}

// Good: Uses 2 storage slots
struct Channel {
    address participant1;      // 20 bytes (slot 1)
    uint96 settlementTimeout;  // 12 bytes (slot 1) - fits with address
    address participant2;      // 20 bytes (slot 2)
    ChannelState state;        // 1 byte (slot 2) - fits with address
    // 11 bytes remaining in slot 2 for future use
}
```

**Savings:** 20,000 gas per additional slot avoided

#### 2. Use Custom Errors (Solidity 0.8.4+)

```solidity
// Bad: String error messages
require(channel.state == ChannelState.Opened, "Channel must be in Opened state");

// Good: Custom errors
error ChannelNotOpened(bytes32 channelId, ChannelState currentState);

if (channel.state != ChannelState.Opened) {
    revert ChannelNotOpened(channelId, channel.state);
}
```

**Savings:** ~18,000 gas per error

#### 3. Use Calldata Instead of Memory

```solidity
// Bad: Copies to memory
function processBalanceProof(
    BalanceProof memory proof
) external {
    // ...
}

// Good: Uses calldata (read-only)
function processBalanceProof(
    BalanceProof calldata proof
) external {
    // ...
}
```

**Savings:** ~1,000 gas per 32 bytes

#### 4. Cache Storage Variables

```solidity
// Bad: Multiple SLOAD operations
function settle(bytes32 channelId) external {
    require(channels[channelId].state == ChannelState.Closed);
    require(block.timestamp >= channels[channelId].closedAt + channels[channelId].timeout);

    uint256 amount1 = channels[channelId].participant1Balance;
    uint256 amount2 = channels[channelId].participant2Balance;
}

// Good: Single SLOAD, cache in memory
function settle(bytes32 channelId) external {
    Channel storage channel = channels[channelId];  // Single SLOAD for reference
    require(channel.state == ChannelState.Closed);
    require(block.timestamp >= channel.closedAt + channel.timeout);

    uint256 amount1 = channel.participant1Balance;
    uint256 amount2 = channel.participant2Balance;
}
```

**Savings:** ~2,100 gas per avoided SLOAD

#### 5. Immutable Contract References

```solidity
// Bad: Storage variable
IERC20 public token;

constructor(address _token) {
    token = IERC20(_token);  // SSTORE
}

// Good: Immutable (stored in contract code)
IERC20 public immutable token;

constructor(address _token) {
    token = IERC20(_token);  // No SSTORE, baked into bytecode
}
```

**Savings:** 2,100 gas per read after deployment

#### 6. Batch Operations

```solidity
// Allow opening channel + initial deposit in one transaction
function openChannelWithDeposit(
    address partner,
    uint256 settlementTimeout,
    uint256 depositAmount
) external {
    bytes32 channelId = openChannel(partner, settlementTimeout);
    deposit(channelId, depositAmount);
}
```

**Savings:** ~21,000 gas (one less transaction's base cost)

#### 7. Event Indexing Optimization

```solidity
// Use indexed parameters for filtering, but max 3 indexed params
event ChannelOpened(
    bytes32 indexed channelId,
    address indexed participant1,
    address indexed participant2,
    uint256 settlementTimeout  // Not indexed - cheaper
);
```

**Guideline:** Index parameters used for filtering, leave others unindexed

#### 8. Use Mapping Instead of Array for Lookups

```solidity
// Bad: Array lookup O(n)
address[] public participants;

function isParticipant(address addr) public view returns (bool) {
    for (uint i = 0; i < participants.length; i++) {
        if (participants[i] == addr) return true;
    }
    return false;
}

// Good: Mapping lookup O(1)
mapping(address => bool) public isParticipant;
```

**Savings:** Avoids O(n) loop, constant gas

#### 9. Minimize On-Chain Storage

```solidity
// Store balance hash instead of full balance proof
struct ParticipantState {
    uint256 nonce;
    bytes32 balanceHash;  // keccak256(transferredAmount, lockedAmount)
    // Don't store: transferredAmount, lockedAmount, locksRoot
    // These are provided during settlement
}
```

**Savings:** 15,000+ gas per avoided storage slot

#### 10. Use Assembly for Signature Verification (Advanced)

```solidity
// Slightly more gas efficient than ECDSA library for high-frequency ops
function recoverSigner(
    bytes32 hash,
    bytes memory signature
) internal pure returns (address) {
    bytes32 r;
    bytes32 s;
    uint8 v;

    assembly {
        r := mload(add(signature, 32))
        s := mload(add(signature, 64))
        v := byte(0, mload(add(signature, 96)))
    }

    return ecrecover(hash, v, r, s);
}
```

**Savings:** ~200-500 gas vs memory operations

### Trade-offs Between Features and Gas Costs

#### Feature: Virtual Channels

- **Gas Benefit:** Eliminates on-chain operations for intermediate hops
- **Gas Cost:** More complex setup, higher deployment cost
- **Recommendation:** Only if building channel network

#### Feature: Conditional Payments

- **Gas Benefit:** Enables complex payment patterns
- **Gas Cost:** +30,000-50,000 gas for condition verification
- **Recommendation:** Make optional, not required for all channels

#### Feature: Multi-Token Single Contract

- **Gas Benefit:** One deployment for all tokens
- **Gas Cost:** +5,000-10,000 gas per operation (store token address)
- **Recommendation:** Use separate TokenNetwork per token instead

#### Feature: Upgradeable Contracts

- **Gas Benefit:** Can fix bugs and add features
- **Gas Cost:** +20,000-30,000 gas per call (proxy overhead)
- **Recommendation:** Use for registry, not for channels themselves

#### Feature: Emergency Pause

- **Gas Benefit:** Can halt system during attack
- **Gas Cost:** +2,000-3,000 gas per operation (pause check)
- **Recommendation:** Essential for production, worth the cost

### L2 Deployment Considerations

**Arbitrum:**

- Gas costs 10-50x lower than L1
- Deployment: ~$5-10 vs $200-400 on L1
- Same Solidity code, minimal changes
- Consider Arbitrum-specific gas optimizations

**Optimism:**

- Gas costs similar to Arbitrum
- Single-round fraud proofs vs Arbitrum's multi-round
- EVM equivalence makes porting easy
- Slightly higher fees than Arbitrum

**zkSync/StarkNet:**

- Lowest gas costs but different VM
- Requires code adaptation
- Better for high-throughput scenarios
- More complex deployment

**Recommendation:** Deploy on Arbitrum or Optimism for best balance of cost, compatibility, and security

---

## IMPLEMENTATION GUIDANCE

### Recommended Solidity Patterns and Contract Structure

```
contracts/
├── core/
│   ├── TokenNetworkRegistry.sol
│   ├── TokenNetwork.sol
│   └── SecretRegistry.sol (if using hash locks)
├── libraries/
│   ├── ChannelLibrary.sol
│   ├── SignatureValidator.sol
│   └── BalanceProofLibrary.sol
├── interfaces/
│   ├── ITokenNetworkRegistry.sol
│   ├── ITokenNetwork.sol
│   └── IERC20Channel.sol
├── test/
│   ├── mocks/
│   │   ├── MockERC20.sol
│   │   ├── MaliciousERC20.sol (for testing reentrancy)
│   │   └── FeeOnTransferToken.sol (for testing edge cases)
│   └── utils/
│       └── Signatures.sol (helper for creating test signatures)
└── utils/
    └── Pausable.sol (circuit breaker)
```

### Core Contract Implementations

#### TokenNetworkRegistry.sol

```solidity
// SPDX-License-Identifier: MIT
pragma solidity 0.8.20;

import "@openzeppelin/contracts/access/Ownable.sol";
import "./TokenNetwork.sol";

/// @title TokenNetworkRegistry
/// @notice Factory and registry for TokenNetwork contracts
/// @dev One TokenNetwork per ERC20 token
contract TokenNetworkRegistry is Ownable {
    // Token address => TokenNetwork address
    mapping(address => address) public token_to_token_networks;

    // TokenNetwork address => Token address (reverse lookup)
    mapping(address => address) public token_network_to_token;

    // Settlement timeout limits
    uint256 public constant MIN_SETTLEMENT_TIMEOUT = 1 days;
    uint256 public constant MAX_SETTLEMENT_TIMEOUT = 30 days;

    // Events
    event TokenNetworkCreated(address indexed token, address indexed tokenNetwork);

    // Errors
    error TokenNetworkAlreadyExists(address token);
    error InvalidTokenAddress();
    error TokenNetworkCreationFailed();

    /// @notice Create a new TokenNetwork for an ERC20 token
    /// @param token Address of the ERC20 token
    /// @return tokenNetwork Address of the created TokenNetwork
    function createTokenNetwork(address token)
        external
        returns (address tokenNetwork)
    {
        if (token == address(0)) revert InvalidTokenAddress();
        if (token_to_token_networks[token] != address(0)) {
            revert TokenNetworkAlreadyExists(token);
        }

        // Deploy new TokenNetwork
        tokenNetwork = address(new TokenNetwork(
            token,
            MIN_SETTLEMENT_TIMEOUT,
            MAX_SETTLEMENT_TIMEOUT
        ));

        if (tokenNetwork == address(0)) revert TokenNetworkCreationFailed();

        // Register mappings
        token_to_token_networks[token] = tokenNetwork;
        token_network_to_token[tokenNetwork] = token;

        emit TokenNetworkCreated(token, tokenNetwork);
    }

    /// @notice Get TokenNetwork for a token
    /// @param token Address of the ERC20 token
    /// @return Address of the TokenNetwork (or 0 if not exists)
    function getTokenNetwork(address token)
        external
        view
        returns (address)
    {
        return token_to_token_networks[token];
    }
}
```

#### TokenNetwork.sol (Simplified Core)

```solidity
// SPDX-License-Identifier: MIT
pragma solidity 0.8.20;

import "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import "@openzeppelin/contracts/security/ReentrancyGuard.sol";
import "@openzeppelin/contracts/security/Pausable.sol";
import "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";

/// @title TokenNetwork
/// @notice Manages payment channels for a specific ERC20 token
contract TokenNetwork is ReentrancyGuard, Pausable {
    using SafeERC20 for IERC20;

    // Token for this network
    IERC20 public immutable token;

    // Settlement timeout limits
    uint256 public immutable minSettlementTimeout;
    uint256 public immutable maxSettlementTimeout;

    // Channel states
    enum ChannelState { NonExistent, Opened, Closed, Settled }

    // Channel data structure
    struct Channel {
        uint256 settlementTimeout;
        ChannelState state;
        uint256 closedAt;
        address closer;
        mapping(address => ParticipantData) participants;
    }

    // Per-participant data
    struct ParticipantData {
        uint256 deposit;
        uint256 withdrawn;
        uint256 nonce;
        bytes32 balanceHash;
    }

    // Channel ID => Channel
    mapping(bytes32 => Channel) public channels;

    // Channel counter for unique IDs
    uint256 public channelCounter;

    // EIP-712 Domain Separator
    bytes32 public immutable DOMAIN_SEPARATOR;

    // EIP-712 Type Hashes
    bytes32 public constant BALANCE_PROOF_TYPEHASH = keccak256(
        "BalanceProof(bytes32 channelId,uint256 nonce,uint256 transferredAmount,uint256 lockedAmount)"
    );

    // Events
    event ChannelOpened(
        bytes32 indexed channelId,
        address indexed participant1,
        address indexed participant2,
        uint256 settlementTimeout
    );

    event ChannelNewDeposit(
        bytes32 indexed channelId,
        address indexed participant,
        uint256 totalDeposit
    );

    event ChannelClosed(
        bytes32 indexed channelId,
        address indexed closer,
        uint256 nonce
    );

    event ChannelSettled(
        bytes32 indexed channelId,
        uint256 participant1Amount,
        uint256 participant2Amount
    );

    // Errors
    error InvalidSettlementTimeout();
    error ChannelAlreadyExists();
    error ChannelDoesNotExist();
    error NotParticipant();
    error InvalidChannelState(ChannelState expected, ChannelState actual);
    error InvalidNonce();
    error InvalidSignature();
    error SettlementTimeoutNotExpired();

    constructor(
        address _token,
        uint256 _minSettlementTimeout,
        uint256 _maxSettlementTimeout
    ) {
        token = IERC20(_token);
        minSettlementTimeout = _minSettlementTimeout;
        maxSettlementTimeout = _maxSettlementTimeout;

        // Initialize EIP-712 domain separator
        DOMAIN_SEPARATOR = keccak256(abi.encode(
            keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"),
            keccak256("PaymentChannel"),
            keccak256("1"),
            block.chainid,
            address(this)
        ));
    }

    /// @notice Open a new payment channel
    /// @param partner Address of the channel partner
    /// @param settlementTimeout Timeout for settlement after closure
    /// @return channelId Unique identifier for the channel
    function openChannel(
        address partner,
        uint256 settlementTimeout
    ) external whenNotPaused nonReentrant returns (bytes32 channelId) {
        if (settlementTimeout < minSettlementTimeout ||
            settlementTimeout > maxSettlementTimeout) {
            revert InvalidSettlementTimeout();
        }

        // Create unique channel ID
        channelId = keccak256(abi.encodePacked(
            msg.sender,
            partner,
            channelCounter++
        ));

        Channel storage channel = channels[channelId];
        if (channel.state != ChannelState.NonExistent) {
            revert ChannelAlreadyExists();
        }

        // Initialize channel
        channel.state = ChannelState.Opened;
        channel.settlementTimeout = settlementTimeout;

        emit ChannelOpened(channelId, msg.sender, partner, settlementTimeout);
    }

    /// @notice Deposit tokens into a channel
    /// @param channelId The channel identifier
    /// @param participant The participant making the deposit
    /// @param amount Amount to deposit
    function deposit(
        bytes32 channelId,
        address participant,
        uint256 amount
    ) external whenNotPaused nonReentrant {
        Channel storage channel = channels[channelId];

        if (channel.state != ChannelState.Opened) {
            revert InvalidChannelState(ChannelState.Opened, channel.state);
        }

        // Measure actual received amount (handles fee-on-transfer tokens)
        uint256 balanceBefore = token.balanceOf(address(this));
        token.safeTransferFrom(msg.sender, address(this), amount);
        uint256 balanceAfter = token.balanceOf(address(this));
        uint256 actualAmount = balanceAfter - balanceBefore;

        // Update deposit
        channel.participants[participant].deposit += actualAmount;

        emit ChannelNewDeposit(
            channelId,
            participant,
            channel.participants[participant].deposit
        );
    }

    /// @notice Close a channel (unilateral)
    /// @param channelId The channel identifier
    /// @param nonce The nonce of the final state
    /// @param transferredAmount The transferred amount
    /// @param lockedAmount The locked amount
    /// @param signature The partner's signature on the balance proof
    function closeChannel(
        bytes32 channelId,
        uint256 nonce,
        uint256 transferredAmount,
        uint256 lockedAmount,
        bytes calldata signature,
        address partner
    ) external whenNotPaused nonReentrant {
        Channel storage channel = channels[channelId];

        if (channel.state != ChannelState.Opened) {
            revert InvalidChannelState(ChannelState.Opened, channel.state);
        }

        // Verify signature
        bytes32 balanceHash = keccak256(abi.encode(
            transferredAmount,
            lockedAmount
        ));

        if (!_verifyBalanceProof(
            channelId,
            nonce,
            transferredAmount,
            lockedAmount,
            signature,
            partner
        )) {
            revert InvalidSignature();
        }

        // Update state
        channel.state = ChannelState.Closed;
        channel.closedAt = block.timestamp;
        channel.closer = msg.sender;
        channel.participants[partner].nonce = nonce;
        channel.participants[partner].balanceHash = balanceHash;

        emit ChannelClosed(channelId, msg.sender, nonce);
    }

    /// @notice Settle a closed channel
    /// @param channelId The channel identifier
    function settleChannel(
        bytes32 channelId,
        address participant1,
        address participant2
    ) external whenNotPaused nonReentrant {
        Channel storage channel = channels[channelId];

        if (channel.state != ChannelState.Closed) {
            revert InvalidChannelState(ChannelState.Closed, channel.state);
        }

        if (block.timestamp < channel.closedAt + channel.settlementTimeout) {
            revert SettlementTimeoutNotExpired();
        }

        // Calculate final balances
        uint256 amount1 = _calculateSettlementAmount(channelId, participant1);
        uint256 amount2 = _calculateSettlementAmount(channelId, participant2);

        // Update state before transfers
        channel.state = ChannelState.Settled;

        // Transfer tokens
        if (amount1 > 0) {
            token.safeTransfer(participant1, amount1);
        }
        if (amount2 > 0) {
            token.safeTransfer(participant2, amount2);
        }

        emit ChannelSettled(channelId, amount1, amount2);
    }

    /// @dev Verify balance proof signature using EIP-712
    function _verifyBalanceProof(
        bytes32 channelId,
        uint256 nonce,
        uint256 transferredAmount,
        uint256 lockedAmount,
        bytes memory signature,
        address signer
    ) internal view returns (bool) {
        bytes32 structHash = keccak256(abi.encode(
            BALANCE_PROOF_TYPEHASH,
            channelId,
            nonce,
            transferredAmount,
            lockedAmount
        ));

        bytes32 digest = keccak256(abi.encodePacked(
            "\x19\x01",
            DOMAIN_SEPARATOR,
            structHash
        ));

        address recovered = ECDSA.recover(digest, signature);
        return recovered == signer && recovered != address(0);
    }

    /// @dev Calculate settlement amount for a participant
    function _calculateSettlementAmount(
        bytes32 channelId,
        address participant
    ) internal view returns (uint256) {
        ParticipantData storage data = channels[channelId].participants[participant];
        // Simplified: deposit - withdrawn - transferred
        // Real implementation would decode balanceHash
        return data.deposit - data.withdrawn;
    }

    /// @notice Emergency pause
    function pause() external onlyOwner {
        _pause();
    }

    /// @notice Unpause
    function unpause() external onlyOwner {
        _unpause();
    }
}
```

### Required Library Dependencies

```json
{
  "dependencies": {
    "@openzeppelin/contracts": "^5.0.0",
    "@openzeppelin/contracts-upgradeable": "^5.0.0"
  },
  "devDependencies": {
    "forge-std": "github:foundry-rs/forge-std",
    "solidity-coverage": "^0.8.5",
    "hardhat": "^2.19.0",
    "@nomicfoundation/hardhat-foundry": "^1.1.1",
    "prettier": "^3.0.0",
    "prettier-plugin-solidity": "^1.1.3",
    "solhint": "^4.0.0"
  }
}
```

**Key Libraries:**

1. **OpenZeppelin Contracts:**
   - `SafeERC20`: Safe token transfers
   - `ReentrancyGuard`: Reentrancy protection
   - `ECDSA`: Signature verification
   - `Pausable`: Circuit breaker
   - `Ownable`: Access control

2. **Foundry Standard Library:**
   - Testing utilities
   - VM cheats for testing
   - Console logging

### Upgradeability Strategy Options

#### Option 1: Immutable Contracts (Recommended for Channels)

**Pros:**

- Simpler, more gas efficient
- No proxy overhead
- Easier to audit
- Users trust immutability

**Cons:**

- Cannot fix bugs after deployment
- Cannot add features
- Must migrate to new contracts

**Best For:** Core TokenNetwork contracts where immutability is a feature

#### Option 2: UUPS Proxy Pattern (Recommended for Registry)

```solidity
import "@openzeppelin/contracts-upgradeable/proxy/utils/UUPSUpgradeable.sol";

contract TokenNetworkRegistryUpgradeable is
    Initializable,
    UUPSUpgradeable,
    OwnableUpgradeable
{
    function initialize() public initializer {
        __Ownable_init();
        __UUPSUpgradeable_init();
    }

    function _authorizeUpgrade(address newImplementation)
        internal
        override
        onlyOwner
    {}
}
```

**Pros:**

- Can fix bugs and add features
- Upgrade logic in implementation
- More gas efficient than Transparent Proxy

**Cons:**

- More complex
- Upgrade risks
- Storage collision risks

**Best For:** TokenNetworkRegistry, administrative contracts

#### Option 3: Diamond Pattern (Not Recommended)

**Cons:**

- Overly complex
- Trail of Bits audit warned against it
- Larger attack surface
- Difficult to audit

**When to Use:** Only if you absolutely need unlimited contract size

### Off-Chain Component Requirements

#### 1. Message Signing Library (JavaScript/TypeScript)

```typescript
import { ethers } from 'ethers';

interface BalanceProof {
  channelId: string;
  nonce: number;
  transferredAmount: bigint;
  lockedAmount: bigint;
}

class PaymentChannelClient {
  private signer: ethers.Signer;
  private tokenNetwork: ethers.Contract;

  async signBalanceProof(proof: BalanceProof): Promise<string> {
    const domain = {
      name: 'PaymentChannel',
      version: '1',
      chainId: await this.signer.getChainId(),
      verifyingContract: this.tokenNetwork.address,
    };

    const types = {
      BalanceProof: [
        { name: 'channelId', type: 'bytes32' },
        { name: 'nonce', type: 'uint256' },
        { name: 'transferredAmount', type: 'uint256' },
        { name: 'lockedAmount', type: 'uint256' },
      ],
    };

    return await this.signer._signTypedData(domain, types, proof);
  }

  async verifyBalanceProof(
    proof: BalanceProof,
    signature: string,
    expectedSigner: string
  ): Promise<boolean> {
    const domain = {
      /* same as above */
    };
    const types = {
      /* same as above */
    };

    const recovered = ethers.utils.verifyTypedData(domain, types, proof, signature);

    return recovered.toLowerCase() === expectedSigner.toLowerCase();
  }
}
```

#### 2. Channel State Management

```typescript
interface ChannelState {
  channelId: string;
  participant1: string;
  participant2: string;
  nonce: number;
  balances: {
    [address: string]: {
      deposit: bigint;
      transferred: bigint;
      locked: bigint;
    };
  };
}

class ChannelManager {
  private channels: Map<string, ChannelState> = new Map();

  updateChannel(channelId: string, newState: Partial<ChannelState>) {
    const current = this.channels.get(channelId);
    this.channels.set(channelId, {
      ...current,
      ...newState,
      nonce: (current?.nonce || 0) + 1,
    });
  }

  async sendPayment(channelId: string, amount: bigint): Promise<BalanceProof> {
    const channel = this.channels.get(channelId);
    if (!channel) throw new Error('Channel not found');

    // Update local state
    channel.balances[myAddress].transferred += amount;
    channel.nonce++;

    // Create and sign balance proof
    const proof: BalanceProof = {
      channelId,
      nonce: channel.nonce,
      transferredAmount: channel.balances[myAddress].transferred,
      lockedAmount: channel.balances[myAddress].locked,
    };

    return proof;
  }
}
```

#### 3. Event Monitoring

```typescript
class EventMonitor {
  private tokenNetwork: ethers.Contract;

  async monitorChannel(channelId: string) {
    // Listen for channel events
    this.tokenNetwork.on('ChannelClosed', (id, closer, nonce) => {
      if (id === channelId) {
        this.handleChannelClosed(id, closer, nonce);
      }
    });

    this.tokenNetwork.on('ChannelSettled', (id, amount1, amount2) => {
      if (id === channelId) {
        this.handleChannelSettled(id, amount1, amount2);
      }
    });
  }

  private async handleChannelClosed(channelId: string, closer: string, nonce: number) {
    // Check if we have a more recent state
    const ourState = this.getChannelState(channelId);

    if (ourState.nonce > nonce) {
      // Submit our more recent state
      await this.updateNonClosingBalanceProof(channelId, ourState);
    }
  }
}
```

### Edge Cases for Non-Standard ERC20 Tokens

#### 1. Fee-on-Transfer Tokens

**Problem:** Contract receives less than specified amount

**Handling:**

```solidity
function deposit(bytes32 channelId, uint256 amount) external {
    uint256 balanceBefore = token.balanceOf(address(this));
    token.safeTransferFrom(msg.sender, address(this), amount);
    uint256 balanceAfter = token.balanceOf(address(this));

    uint256 actualAmount = balanceAfter - balanceBefore;
    channels[channelId].participants[msg.sender].deposit += actualAmount;
}
```

**Documentation:**

```
WARNING: Fee-on-transfer tokens are supported but users will receive
less than the nominal amount. Always check actual deposited amount.
```

#### 2. Rebasing Tokens (e.g., Aave aTokens)

**Problem:** Balances change without transfers

**Handling:**

```
NOT SUPPORTED. Rebasing tokens will cause incorrect settlement amounts.
Use wrapped versions or convert to stable tokens before depositing.
```

**Alternative:** Snapshot balances at deposit time, settle based on shares

#### 3. Tokens Without Return Values (USDT, BNB)

**Problem:** transferFrom doesn't return bool

**Handling:**

```solidity
using SafeERC20 for IERC20;

// SafeERC20 handles both cases:
// - Tokens that return bool
// - Tokens that don't return anything
token.safeTransferFrom(sender, recipient, amount);
```

#### 4. Multiple Entry Points (e.g., USDT)

**Problem:** USDT has both ERC20 and legacy transfer functions

**Handling:** Always use standard ERC20 interface, SafeERC20 handles compatibility

#### 5. Blacklist Tokens (e.g., USDC, USDT)

**Problem:** Token can blacklist addresses, blocking transfers

**Handling:**

```solidity
// Allow emergency withdrawal even if one participant is blacklisted
function emergencyWithdraw(bytes32 channelId) external onlyOwner {
    // Owner can rescue funds if participant blacklisted
    // Requires governance approval and time delay
}
```

**Documentation:**

```
WARNING: If either participant is blacklisted by the token contract,
channel settlement may fail. Use emergency withdrawal procedure.
```

---

## CODE EXAMPLES AND PSEUDOCODE

### 1. Channel Creation

```solidity
/**
 * @notice Create and fund a new payment channel
 * @dev Combines channel creation and initial deposit in one transaction
 */
function openChannelWithDeposit(
    address partner,
    uint256 settlementTimeout,
    uint256 depositAmount
) external nonReentrant whenNotPaused returns (bytes32 channelId) {
    // Validate inputs
    require(partner != address(0) && partner != msg.sender, "Invalid partner");
    require(
        settlementTimeout >= MIN_SETTLEMENT_TIMEOUT &&
        settlementTimeout <= MAX_SETTLEMENT_TIMEOUT,
        "Invalid settlement timeout"
    );
    require(depositAmount >= MIN_DEPOSIT, "Deposit too small");

    // Generate unique channel ID
    channelId = keccak256(abi.encodePacked(
        msg.sender,
        partner,
        block.timestamp,
        channelCounter++
    ));

    // Initialize channel
    Channel storage channel = channels[channelId];
    channel.state = ChannelState.Opened;
    channel.settlementTimeout = settlementTimeout;
    channel.participants[msg.sender].isParticipant = true;
    channel.participants[partner].isParticipant = true;

    // Measure actual transfer amount (handles fee-on-transfer)
    uint256 balanceBefore = token.balanceOf(address(this));
    token.safeTransferFrom(msg.sender, address(this), depositAmount);
    uint256 balanceAfter = token.balanceOf(address(this));

    uint256 actualDeposit = balanceAfter - balanceBefore;
    channel.participants[msg.sender].deposit = actualDeposit;

    emit ChannelOpened(channelId, msg.sender, partner, settlementTimeout);
    emit ChannelNewDeposit(channelId, msg.sender, actualDeposit);

    return channelId;
}
```

### 2. Off-Chain Payment Updates

```typescript
/**
 * Off-chain payment flow between Alice and Bob
 */
class PaymentChannelOffChain {
  private channelState: ChannelState;
  private signer: ethers.Signer;

  /**
   * Alice sends payment to Bob off-chain
   */
  async sendPayment(amount: bigint, recipient: string): Promise<SignedBalanceProof> {
    // Update local state
    this.channelState.nonce++;
    this.channelState.myTransferred += amount;

    // Create balance proof
    const balanceProof: BalanceProof = {
      channelId: this.channelState.channelId,
      nonce: this.channelState.nonce,
      transferredAmount: this.channelState.myTransferred,
      lockedAmount: this.channelState.myLocked,
    };

    // Sign using EIP-712
    const signature = await this.signBalanceProof(balanceProof);

    // Send to counterparty off-chain (WebSocket, HTTP, etc.)
    await this.sendToCounterparty({
      balanceProof,
      signature,
    });

    return { balanceProof, signature };
  }

  /**
   * Bob receives and validates payment from Alice
   */
  async receivePayment(signedProof: SignedBalanceProof, sender: string): Promise<boolean> {
    // Verify signature
    const isValid = await this.verifyBalanceProof(
      signedProof.balanceProof,
      signedProof.signature,
      sender
    );

    if (!isValid) {
      throw new Error('Invalid signature');
    }

    // Verify nonce is increasing
    if (signedProof.balanceProof.nonce <= this.channelState.partnerNonce) {
      throw new Error('Nonce not increasing');
    }

    // Verify transferred amount is increasing
    const previousTransferred = this.channelState.partnerTransferred;
    const newTransferred = signedProof.balanceProof.transferredAmount;

    if (newTransferred < previousTransferred) {
      throw new Error('Transferred amount decreased');
    }

    // Calculate actual payment
    const paymentAmount = newTransferred - previousTransferred;

    // Update local state
    this.channelState.partnerNonce = signedProof.balanceProof.nonce;
    this.channelState.partnerTransferred = newTransferred;
    this.channelState.myBalance += paymentAmount;

    // Store proof for potential on-chain submission
    this.storeLatestProof(signedProof);

    console.log(`Received payment of ${paymentAmount} tokens`);
    return true;
  }

  /**
   * Sign balance proof using EIP-712
   */
  private async signBalanceProof(proof: BalanceProof): Promise<string> {
    const domain = {
      name: 'PaymentChannel',
      version: '1',
      chainId: await this.signer.getChainId(),
      verifyingContract: this.tokenNetworkAddress,
    };

    const types = {
      BalanceProof: [
        { name: 'channelId', type: 'bytes32' },
        { name: 'nonce', type: 'uint256' },
        { name: 'transferredAmount', type: 'uint256' },
        { name: 'lockedAmount', type: 'uint256' },
      ],
    };

    return await this.signer._signTypedData(domain, types, proof);
  }
}

/**
 * Example usage: Alice sends 10 tokens to Bob
 */
async function examplePaymentFlow() {
  const alice = new PaymentChannelOffChain(aliceSigner, channelId);
  const bob = new PaymentChannelOffChain(bobSigner, channelId);

  // Alice sends 10 tokens to Bob
  const signedProof = await alice.sendPayment(ethers.utils.parseEther('10'), bobAddress);

  // Bob receives and validates
  await bob.receivePayment(signedProof, aliceAddress);

  // Bob can now send back or continue receiving
  // All off-chain, no gas costs!
}
```

### 3. Channel Closure and Settlement

```solidity
/**
 * @notice Close channel cooperatively with both signatures
 * @dev Most gas-efficient closure method
 */
function cooperativeClose(
    bytes32 channelId,
    address participant1,
    address participant2,
    uint256 participant1Amount,
    uint256 participant2Amount,
    bytes calldata signature1,
    bytes calldata signature2
) external nonReentrant whenNotPaused {
    Channel storage channel = channels[channelId];
    require(channel.state == ChannelState.Opened, "Channel not open");

    // Verify both participants signed the final settlement
    bytes32 settlementHash = keccak256(abi.encodePacked(
        channelId,
        participant1Amount,
        participant2Amount
    ));

    bytes32 ethSignedHash = ECDSA.toEthSignedMessageHash(settlementHash);

    require(
        ECDSA.recover(ethSignedHash, signature1) == participant1,
        "Invalid signature 1"
    );
    require(
        ECDSA.recover(ethSignedHash, signature2) == participant2,
        "Invalid signature 2"
    );

    // Verify amounts don't exceed deposits
    uint256 totalDeposit =
        channel.participants[participant1].deposit +
        channel.participants[participant2].deposit;

    require(
        participant1Amount + participant2Amount <= totalDeposit,
        "Settlement exceeds deposits"
    );

    // Update state before transfers (checks-effects-interactions)
    channel.state = ChannelState.Settled;

    // Transfer final balances
    if (participant1Amount > 0) {
        token.safeTransfer(participant1, participant1Amount);
    }
    if (participant2Amount > 0) {
        token.safeTransfer(participant2, participant2Amount);
    }

    emit ChannelSettled(channelId, participant1Amount, participant2Amount);
}

/**
 * @notice Close channel unilaterally (one party initiates)
 * @dev Triggers challenge window
 */
function closeChannelUnilateral(
    bytes32 channelId,
    uint256 nonce,
    uint256 transferredAmount,
    uint256 lockedAmount,
    bytes calldata partnerSignature,
    address partner
) external nonReentrant whenNotPaused {
    Channel storage channel = channels[channelId];
    require(channel.state == ChannelState.Opened, "Channel not open");
    require(
        channel.participants[msg.sender].isParticipant,
        "Not a participant"
    );

    // Verify partner's signature on balance proof
    bytes32 balanceHash = keccak256(abi.encode(
        transferredAmount,
        lockedAmount
    ));

    require(
        _verifyBalanceProof(
            channelId,
            nonce,
            transferredAmount,
            lockedAmount,
            partnerSignature,
            partner
        ),
        "Invalid signature"
    );

    // Store closing state
    channel.state = ChannelState.Closed;
    channel.closedAt = block.timestamp;
    channel.closer = msg.sender;
    channel.participants[partner].nonce = nonce;
    channel.participants[partner].balanceHash = balanceHash;

    emit ChannelClosed(channelId, msg.sender, nonce);
}

/**
 * @notice Submit newer balance proof during challenge window
 * @dev Prevents closure with stale state
 */
function updateNonClosingBalanceProof(
    bytes32 channelId,
    uint256 nonce,
    uint256 transferredAmount,
    uint256 lockedAmount,
    bytes calldata closerSignature,
    address closer
) external nonReentrant {
    Channel storage channel = channels[channelId];
    require(channel.state == ChannelState.Closed, "Channel not closed");
    require(channel.closer != msg.sender, "Closer cannot update");
    require(
        block.timestamp < channel.closedAt + channel.settlementTimeout,
        "Challenge period expired"
    );

    // Verify this is a newer state
    require(
        nonce > channel.participants[closer].nonce,
        "Nonce not higher"
    );

    // Verify closer's signature
    require(
        _verifyBalanceProof(
            channelId,
            nonce,
            transferredAmount,
            lockedAmount,
            closerSignature,
            closer
        ),
        "Invalid signature"
    );

    // Update with newer state
    bytes32 balanceHash = keccak256(abi.encode(
        transferredAmount,
        lockedAmount
    ));

    channel.participants[closer].nonce = nonce;
    channel.participants[closer].balanceHash = balanceHash;

    emit NonClosingBalanceProofUpdated(channelId, closer, nonce);
}

/**
 * @notice Settle channel after challenge period
 */
function settleChannel(
    bytes32 channelId,
    address participant1,
    uint256 participant1Transferred,
    uint256 participant1Locked,
    address participant2,
    uint256 participant2Transferred,
    uint256 participant2Locked
) external nonReentrant {
    Channel storage channel = channels[channelId];
    require(channel.state == ChannelState.Closed, "Channel not closed");
    require(
        block.timestamp >= channel.closedAt + channel.settlementTimeout,
        "Challenge period not ended"
    );

    // Verify balance hashes match stored values
    bytes32 balanceHash1 = keccak256(abi.encode(
        participant1Transferred,
        participant1Locked
    ));
    bytes32 balanceHash2 = keccak256(abi.encode(
        participant2Transferred,
        participant2Locked
    ));

    require(
        channel.participants[participant1].balanceHash == balanceHash1,
        "Balance hash 1 mismatch"
    );
    require(
        channel.participants[participant2].balanceHash == balanceHash2,
        "Balance hash 2 mismatch"
    );

    // Calculate final amounts
    uint256 amount1 = channel.participants[participant1].deposit -
                      participant1Transferred +
                      participant2Transferred;

    uint256 amount2 = channel.participants[participant2].deposit -
                      participant2Transferred +
                      participant1Transferred;

    // Update state before transfers
    channel.state = ChannelState.Settled;

    // Transfer final amounts
    if (amount1 > 0) {
        token.safeTransfer(participant1, amount1);
    }
    if (amount2 > 0) {
        token.safeTransfer(participant2, amount2);
    }

    emit ChannelSettled(channelId, amount1, amount2);
}
```

### 4. Dispute Resolution

```typescript
/**
 * Off-chain client monitors for channel closure and disputes
 */
class DisputeMonitor {
  private tokenNetwork: ethers.Contract;
  private channelState: ChannelState;

  /**
   * Monitor for ChannelClosed events
   */
  async startMonitoring(channelId: string) {
    this.tokenNetwork.on('ChannelClosed', async (id: string, closer: string, nonce: number) => {
      if (id === channelId) {
        await this.handleChannelClosed(id, closer, nonce);
      }
    });
  }

  /**
   * Handle channel closure by counterparty
   */
  private async handleChannelClosed(channelId: string, closer: string, closingNonce: number) {
    console.log(`Channel ${channelId} closed by ${closer} with nonce ${closingNonce}`);

    // Get our latest state
    const ourLatestProof = this.getLatestBalanceProof(channelId);

    // Check if we have a more recent state
    if (ourLatestProof.nonce > closingNonce) {
      console.warn(
        `Detected stale state closure! Our nonce: ${ourLatestProof.nonce}, closing nonce: ${closingNonce}`
      );

      // Submit our more recent state
      await this.challengeClose(channelId, ourLatestProof);
    } else {
      console.log('Closure state is current, waiting for settlement period');
    }
  }

  /**
   * Challenge closure with more recent state
   */
  private async challengeClose(channelId: string, newerProof: SignedBalanceProof) {
    console.log('Challenging channel closure with newer state...');

    const tx = await this.tokenNetwork.updateNonClosingBalanceProof(
      channelId,
      newerProof.balanceProof.nonce,
      newerProof.balanceProof.transferredAmount,
      newerProof.balanceProof.lockedAmount,
      newerProof.signature,
      this.partnerAddress
    );

    await tx.wait();
    console.log('Successfully challenged closure!');
  }

  /**
   * Automatically settle after challenge period
   */
  async autoSettle(channelId: string) {
    const channel = await this.tokenNetwork.channels(channelId);
    const settlementTime = channel.closedAt.add(channel.settlementTimeout);

    // Wait for settlement period
    const now = Math.floor(Date.now() / 1000);
    const waitTime = settlementTime.toNumber() - now;

    if (waitTime > 0) {
      console.log(`Waiting ${waitTime} seconds for settlement period...`);
      await new Promise((resolve) => setTimeout(resolve, waitTime * 1000));
    }

    // Settle channel
    const ourState = this.channelState;
    const partnerState = this.partnerChannelState;

    const tx = await this.tokenNetwork.settleChannel(
      channelId,
      this.myAddress,
      ourState.transferred,
      ourState.locked,
      this.partnerAddress,
      partnerState.transferred,
      partnerState.locked
    );

    await tx.wait();
    console.log('Channel settled successfully!');
  }
}

/**
 * Example: Complete dispute resolution flow
 */
async function exampleDisputeFlow() {
  const monitor = new DisputeMonitor(tokenNetworkContract, channelId);

  // Start monitoring for closure events
  await monitor.startMonitoring(channelId);

  // If counterparty closes with old state, challenge is automatic
  // After challenge period, settle automatically
  await monitor.autoSettle(channelId);
}
```

### 5. Multi-Token Handling

```solidity
/**
 * @notice Registry managing multiple token networks
 */
contract TokenNetworkRegistry {
    // Token => TokenNetwork mapping
    mapping(address => address) public token_to_token_networks;

    /**
     * Create new TokenNetwork for a token
     */
    function createTokenNetwork(address token)
        external
        returns (address tokenNetwork)
    {
        require(token != address(0), "Invalid token");
        require(
            token_to_token_networks[token] == address(0),
            "Network already exists"
        );

        // Deploy new TokenNetwork contract
        tokenNetwork = address(new TokenNetwork(
            token,
            MIN_SETTLEMENT_TIMEOUT,
            MAX_SETTLEMENT_TIMEOUT
        ));

        // Register
        token_to_token_networks[token] = tokenNetwork;

        emit TokenNetworkCreated(token, tokenNetwork);
    }
}

/**
 * Off-chain: Managing channels across multiple tokens
 */
class MultiTokenChannelManager {
    private tokenNetworks: Map<string, ethers.Contract> = new Map();
    private channels: Map<string, Map<string, ChannelState>> = new Map();

    /**
     * Initialize channel for a specific token
     */
    async openChannelForToken(
        tokenAddress: string,
        partner: string,
        depositAmount: bigint
    ): Promise<string> {
        // Get or create TokenNetwork for this token
        let tokenNetwork = this.tokenNetworks.get(tokenAddress);

        if (!tokenNetwork) {
            // Get TokenNetwork from registry
            const networkAddress = await this.registry.token_to_token_networks(
                tokenAddress
            );

            if (networkAddress === ethers.constants.AddressZero) {
                // Create new TokenNetwork
                const tx = await this.registry.createTokenNetwork(tokenAddress);
                await tx.wait();

                const newNetworkAddress = await this.registry.token_to_token_networks(
                    tokenAddress
                );
                tokenNetwork = new ethers.Contract(
                    newNetworkAddress,
                    TokenNetworkABI,
                    this.signer
                );
            } else {
                tokenNetwork = new ethers.Contract(
                    networkAddress,
                    TokenNetworkABI,
                    this.signer
                );
            }

            this.tokenNetworks.set(tokenAddress, tokenNetwork);
        }

        // Approve token spending
        const token = new ethers.Contract(
            tokenAddress,
            ERC20ABI,
            this.signer
        );
        await token.approve(tokenNetwork.address, depositAmount);

        // Open channel with deposit
        const tx = await tokenNetwork.openChannelWithDeposit(
            partner,
            DEFAULT_SETTLEMENT_TIMEOUT,
            depositAmount
        );
        const receipt = await tx.wait();

        // Extract channel ID from event
        const event = receipt.events?.find(e => e.event === 'ChannelOpened');
        const channelId = event?.args?.channelId;

        // Initialize local state
        if (!this.channels.has(tokenAddress)) {
            this.channels.set(tokenAddress, new Map());
        }

        this.channels.get(tokenAddress)!.set(channelId, {
            channelId,
            tokenAddress,
            partner,
            myDeposit: depositAmount,
            nonce: 0,
            transferred: 0n,
            locked: 0n
        });

        return channelId;
    }

    /**
     * Send payment in specific token
     */
    async sendPayment(
        tokenAddress: string,
        channelId: string,
        amount: bigint
    ): Promise<SignedBalanceProof> {
        const tokenChannels = this.channels.get(tokenAddress);
        const channel = tokenChannels?.get(channelId);

        if (!channel) {
            throw new Error('Channel not found');
        }

        // Update state
        channel.nonce++;
        channel.transferred += amount;

        // Create and sign balance proof
        const proof: BalanceProof = {
            channelId,
            nonce: channel.nonce,
            transferredAmount: channel.transferred,
            lockedAmount: channel.locked
        };

        const tokenNetwork = this.tokenNetworks.get(tokenAddress)!;
        const signature = await this.signBalanceProof(
            proof,
            tokenNetwork.address
        );

        return { balanceProof: proof, signature };
    }
}

/**
 * Example: Multi-token payment flow
 */
async function exampleMultiTokenFlow() {
    const manager = new MultiTokenChannelManager(registryContract, signer);

    // Open channels for different tokens
    const usdcChannel = await manager.openChannelForToken(
        USDC_ADDRESS,
        bobAddress,
        ethers.utils.parseUnits('1000', 6)  // 1000 USDC
    );

    const daiChannel = await manager.openChannelForToken(
        DAI_ADDRESS,
        bobAddress,
        ethers.utils.parseEther('1000')  // 1000 DAI
    );

    // Send payments in different tokens
    await manager.sendPayment(
        USDC_ADDRESS,
        usdcChannel,
        ethers.utils.parseUnits('10', 6)  // 10 USDC
    );

    await manager.sendPayment(
        DAI_ADDRESS,
        daiChannel,
        ethers.utils.parseEther('10')  // 10 DAI
    );
}
```

---

## SOURCE DOCUMENTATION

### Primary Resources

#### XRP Payment Channels

1. **Official Documentation:** https://xrpl.org/docs/concepts/payment-types/payment-channels
   - Complete specification of XRP payment channels
   - Claim-based settlement model
   - Expiry and settlement delay mechanics

2. **Payment Channel Methods API:** https://xrpl.org/docs/references/http-websocket-apis/public-api-methods/payment-channel-methods
   - `channel_authorize` and `channel_verify` methods
   - Signature verification patterns

#### Academic Research

3. **General State Channel Networks (2018):** https://eprint.iacr.org/2018/320.pdf
   - Dziembowski, Faust, Hostakova
   - Formal security definitions
   - Dispute resolution mechanisms
   - Foundational theoretical framework

4. **Perun: Virtual Payment Hubs (2017):** https://eprint.iacr.org/2017/635.pdf
   - Virtual channel concept
   - Multi-party channels
   - Hub-and-spoke topology

5. **Multi-Party Virtual State Channels (2019):** https://eprint.iacr.org/2019/571.pdf
   - Extended virtual channel concepts
   - Multi-party support

6. **Sprites and State Channels (2017):** https://arxiv.org/pdf/1702.05812
   - Payment networks faster than Lightning
   - Dispute resolution optimization

#### Ethereum Implementations

7. **Raiden Network Contracts:** https://github.com/raiden-network/raiden-contracts
   - Production-ready Solidity implementation
   - TokenNetwork pattern
   - Comprehensive test suite

8. **Raiden Specification:** https://raiden-network-specification.readthedocs.io/en/latest/smart_contracts.html
   - Detailed smart contract specification
   - Channel lifecycle documentation
   - Security mechanisms

9. **Celer Network Documentation:** https://celer.network/docs/celercore/channel/overview.html
   - Duplex channel model
   - Conditional payments
   - Settlement mechanisms

10. **Celer Pay Contracts:** https://celer.network/docs/celercore/channel/pay_contracts.html
    - Smart contract architecture
    - API specifications

11. **Celer GitHub:** https://github.com/celer-network/cChannel-eth
    - Ethereum implementation
    - Solidity code

12. **Connext Audits:** https://github.com/connext/audits
    - Multiple professional security audits
    - 2023-2024 audit reports from 0xMacro

13. **Perun GitHub Demo:** https://github.com/perun-network/perun-eth-demo
    - Proof-of-concept implementation
    - CLI payment channel node

#### Code Examples

14. **Simple Payment Channel (Miguel Mota):** https://github.com/miguelmota/sol-payment-channels
    - Unidirectional payment channel example
    - Learning resource

15. **Direct Payment Channels:** https://github.com/jfdelgad/Direct-Payment-channels-over-Ethereum
    - Simple implementation
    - Educational resource

16. **Program the Blockchain Tutorial:** https://programtheblockchain.com/posts/2018/02/23/writing-a-simple-payment-channel/
    - Step-by-step guide
    - Code examples with explanations

#### Security Resources

17. **OpenZeppelin SafeERC20:** https://github.com/OpenZeppelin/openzeppelin-contracts/blob/master/contracts/token/ERC20/utils/SafeERC20.sol
    - Safe token interaction patterns
    - Handle non-standard ERC20s

18. **ECDSA Security:** https://github.com/obheda12/Solidity-Security-Compendium/blob/main/days/day12.md
    - Signature verification pitfalls
    - Malleability issues

19. **Smart Contract Security Field Guide:** https://scsfg.io/hackers/signature-attacks/
    - Signature-related attacks
    - Comprehensive threat catalog

20. **EIP-712 Specification:** https://eips.ethereum.org/EIPS/eip-712
    - Typed structured data signing
    - Domain separation

#### Gas Optimization

21. **State Channels Gas Optimization:** https://blog.statechannels.org/gas-optimizations/gas-optimizations/
    - Optimization techniques
    - Benchmarks

22. **RareSkills Gas Optimization:** https://rareskills.io/post/gas-optimization
    - 80+ optimization tips
    - Solidity best practices

23. **Alchemy Gas Optimization:** https://www.alchemy.com/overviews/solidity-gas-optimization
    - 12 key techniques
    - Practical examples

#### Standards and EIPs

24. **EIP-712:** https://eips.ethereum.org/EIPS/eip-712
    - Typed structured data hashing and signing

25. **EIP-2535 (Diamond Standard):** https://eips.ethereum.org/EIPS/eip-2535
    - Multi-facet proxy pattern
    - (Note: Trail of Bits warns against complexity)

26. **EIP-1967 (Proxy Storage Slots):** https://eips.ethereum.org/EIPS/eip-1967
    - Standard proxy storage patterns

27. **EIP-2612 (Permit):** https://eips.ethereum.org/EIPS/eip-2612
    - ERC20 gasless approvals

#### Layer 2 Information

28. **L2Fees.info:** https://l2fees.info/
    - Real-time L2 gas cost comparison
    - Arbitrum vs Optimism benchmarks

29. **Arbitrum Documentation:** https://docs.arbitrum.io/
    - Deployment guides
    - Gas optimization specifics

30. **Optimism Documentation:** https://community.optimism.io/docs/
    - EVM equivalence
    - Migration guides

### Most Relevant GitHub Repositories

1. **raiden-network/raiden-contracts** (★467)
   - Production payment channel implementation
   - Best reference for TokenNetwork pattern
   - Multiple audits, production-tested

2. **celer-network/cChannel-eth** (★196)
   - Duplex channel implementation
   - Conditional payments
   - Well-documented

3. **perun-network/perun-eth-demo** (★89)
   - Virtual channel implementation
   - Academic rigor
   - Multi-party support

4. **connext/audits** (★23)
   - Recent security audits (2023-2024)
   - Professional audit reports
   - Security best practices

5. **miguelmota/sol-payment-channels** (★156)
   - Simple educational example
   - Good starting point
   - Clear code

6. **OpenZeppelin/openzeppelin-contracts** (★24.3k)
   - Essential security libraries
   - SafeERC20, ECDSA, ReentrancyGuard
   - Industry standard

### Important Audit Reports

1. **Connext A-7 Audit (0xMacro, June 2024):**
   - URL: https://0xmacro.com/library/audits/connext-7
   - Findings on L1XERC20Gateway
   - Best practices for bridge contracts

2. **Connext A-4 Audit (0xMacro, October 2023):**
   - URL: https://0xmacro.com/library/audits/connext-4
   - Core protocol security review
   - Comprehensive findings

3. **Raiden Network Audits:**
   - Multiple audits over project lifetime
   - Available in raiden-contracts repository
   - Historical security evolution

4. **State Channels Vector Audit:**
   - URL: https://medium.com/connext/audit-results-launch-plan-961411801388
   - Audit results and rollout plan
   - Production deployment considerations

### Key Technical Papers

1. **"General State Channel Networks"** - ACM CCS 2018
   - https://dl.acm.org/doi/10.1145/3243734.3243856
   - Foundational state channel security

2. **"Sprites and State Channels: Payment Networks that Go Faster Than Lightning"**
   - https://arxiv.org/pdf/1702.05812
   - Performance optimizations

3. **"Programmable Payment Channels"** - ePrint 2023
   - https://eprint.iacr.org/2023/347.pdf
   - Advanced channel capabilities

4. **"Bitcoin Lightning Network"** - Poon & Dryja 2016
   - Foundational HTLC concepts
   - Cross-blockchain relevance

### Additional Resources

- **Ethereum.org State Channels:** https://ethereum.org/developers/docs/scaling/state-channels/
- **EthHub Payment Channels:** https://docs.ethhub.io/ethereum-roadmap/layer-2-scaling/payment-channels/
- **Blockchain Patterns:** https://research.csiro.au/blockchainpatterns/general-patterns/blockchain-payment-patterns/payment-channel/

---

## RESEARCH GAPS AND AREAS NEEDING FURTHER INVESTIGATION

### Identified Gaps

1. **Production Gas Cost Benchmarks:**
   - Limited recent data on actual gas costs in production
   - L2 deployment costs not extensively documented
   - Need real-world measurement data

2. **Cross-L2 Compatibility:**
   - How channels behave across different L2s
   - Bridge integration patterns unclear
   - Limited research on multi-L2 deployments

3. **MEV Considerations:**
   - Front-running during channel closure
   - MEV impact on challenge windows
   - Protection mechanisms not well documented

4. **Large-Scale Network Effects:**
   - Channel network topologies in production
   - Routing efficiency at scale
   - Economic incentives for intermediaries

5. **Privacy Enhancements:**
   - Zero-knowledge proofs for balance privacy
   - Anonymous payment routing
   - Privacy-preserving dispute resolution

6. **Governance and Upgrades:**
   - On-chain governance for channel parameters
   - Safe upgrade paths for active channels
   - Migration strategies

7. **Insurance and Collateral:**
   - Insurance mechanisms for channel failures
   - Collateral optimization strategies
   - Risk assessment frameworks

### Recommended Further Research

1. **Benchmarking Study:**
   - Deploy test channels on mainnet and multiple L2s
   - Measure actual gas costs across operations
   - Compare different architectural patterns

2. **Security Formal Verification:**
   - Complete formal verification using Certora or K
   - Prove key invariants (fund conservation, etc.)
   - Verify state machine correctness

3. **Economic Modeling:**
   - Game theory analysis of channel economics
   - Optimal fee structures
   - Griefing incentives and penalties

4. **Privacy Integration:**
   - Research ZK-SNARK integration
   - Anonymous routing protocols
   - Private balance proofs

5. **Cross-Chain Channels:**
   - Atomic swaps via channels
   - Cross-chain settlement
   - Bridge integration patterns

---

## CONCLUSION

This comprehensive research provides a solid foundation for implementing XRP-style payment channels as EVM smart contracts with multi-token and multi-user support. The recommended architecture draws from proven production implementations (Raiden, Celer, Connext, Perun) while addressing the specific requirements of multi-token support and EVM compatibility.

**Key Takeaways:**

1. **Use Separate TokenNetwork per Token:** This is the proven pattern from Raiden and provides the best balance of security, gas efficiency, and maintainability.

2. **Security is Paramount:** Implement all recommended mitigations for reentrancy, signature replay, non-standard tokens, and griefing attacks. Use OpenZeppelin libraries extensively.

3. **EIP-712 for Signatures:** Mandatory for secure, user-friendly signatures with proper domain separation.

4. **Challenge Windows are Essential:** Protect against stale state submission with configurable challenge periods and penalty mechanisms.

5. **Gas Optimization Matters:** Use modern Solidity patterns (custom errors, immutable, calldata) and carefully optimize storage layout.

6. **Testing is Critical:** Extensive fuzzing, unit tests, integration tests, and professional audits are non-negotiable for production deployment.

7. **Plan for Edge Cases:** Non-standard ERC20 tokens require special handling. Document limitations clearly.

8. **L2 Deployment Recommended:** Consider deploying on Arbitrum or Optimism for 10-100x gas cost reduction while maintaining security.

The estimated 13-18 week development timeline includes adequate time for security hardening and professional auditing. This is essential given the financial nature of payment channels and the complexity of the attack surface.

With proper implementation following the patterns and security measures outlined in this research, XRP-style payment channels can be successfully deployed on EVM-compatible blockchains, enabling efficient off-chain microtransactions with on-chain settlement guarantees.

---

**End of Research Report**

_For questions or clarifications about this research, please refer to the source documentation links provided throughout this document._
