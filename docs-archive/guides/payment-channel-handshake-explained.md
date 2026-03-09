# ILP Payment Channel "Handshake" - Explained

## Quick Answer

**Q: What does ILP use for payment channel handshake between connectors?**

**A: BTP `payment-channel-claim` sub-protocol (Epic 17), NOT SPSP.**

---

## What is SPSP? (RFC-0009)

**SPSP (Simple Payment Setup Protocol)** is an **Application Layer** protocol using HTTPS to set up end-user payments:

### SPSP Purpose

- **End-user payment setup** (Alice → Bob)
- Resolving payment pointers ($alice@wallet.example)
- Getting receiver's ILP address and shared secret
- Setting up STREAM connections for payments

### SPSP Flow

```
Sender                    Receiver's Server
──────                    ─────────────────
1. Resolve payment pointer
   GET https://wallet.example/.well-known/pay/$alice
                              ──────────▶
                              ◀──────────
                              {
                                "destination_account": "g.wallet.alice",
                                "shared_secret": "base64...",
                                "receipts_enabled": true
                              }

2. Use STREAM to send payment to destination_account
```

**SPSP is for end-users, NOT for connector-to-connector settlement.**

---

## What M2M Uses: BTP + Epic 17

### BTP (Bilateral Transfer Protocol - RFC-0023)

**BTP provides the transport layer** for connector communication:

- **Protocol:** WebSocket (bidirectional)
- **Authentication:** Shared secrets (BTP*PEER*\*\_SECRET)
- **Multiplexing:** Multiple sub-protocols over one connection

**BTP Sub-Protocols:**

```typescript
{
  protocolData: [
    {
      protocolName: 'ilp',                    // RFC-0027 packets
      contentType: 0,
      data: <ILP PREPARE/FULFILL/REJECT>
    },
    {
      protocolName: 'payment-channel-claim',  // Epic 17 (M2M extension)
      contentType: 1,                         // application/json
      data: <balance proof>
    }
  ]
}
```

### Epic 17: Payment Channel Claim Exchange

**This is the actual "handshake" for payment channels:**

#### Claim Message Structure

```typescript
// Epic 17: BTPClaimMessage (sent via BTP)
interface EVMClaimMessage {
  version: '1.0';
  blockchain: 'evm';
  messageId: 'claim_abc123';
  timestamp: '2026-02-03T...';
  senderId: 'peer1';

  // Payment channel details
  channelId: '0xabc...'; // Channel identifier
  nonce: 42; // Monotonically increasing
  transferredAmount: '5000000'; // Cumulative sent
  lockedAmount: '0'; // Pending HTLCs
  locksRoot: '0x000...'; // Merkle root

  // Cryptographic proof
  signature: '0x...'; // secp256k1 signature
  additionalHash: '0x...'; // Hash for verification
  publicKey: '0x...'; // For signature verification
}
```

#### Claim Exchange Flow

```
Connector A (Sender)                  Connector B (Receiver)
────────────────────                  ──────────────────────

Packets forwarded: 1,000,000 units
Settlement threshold reached
         ↓
Create balance proof:
  - channelId (from previous setup)
  - nonce (increment from last)
  - transferredAmount (cumulative)
         ↓
Sign with private key (secp256k1)
         ↓
Serialize to JSON
         ↓
Send via BTP WebSocket
  protocolName: 'payment-channel-claim'
  contentType: 1 (JSON)
  data: <claim message>
         ↓
                                      Receive BTP message
                                               ↓
                                      Parse JSON claim
                                               ↓
                                      Verify signature
                                        - Check public key
                                        - Verify secp256k1 signature
                                        - Validate nonce > previous
                                        - Check amount ≤ channel capacity
                                               ↓
                                      Store in SQLite database
                                        - channelId
                                        - nonce
                                        - transferredAmount
                                        - signature
                                               ↓
                                      Emit CLAIM_RECEIVED telemetry
                                               ↓
                                      ClaimRedemptionService (polls every 30s)
                                        - Check if profitable to redeem
                                        - If yes: Submit to blockchain
                                        - If no: Wait for more claims
```

---

## Three Phases of Payment Channel Communication

### Phase 1: Initial Setup (Out-of-Band)

**How:** Manual configuration before deployment

```env
# Static peer-to-blockchain address mapping
PEER1_EVM_ADDRESS=0x...
PEER2_EVM_ADDRESS=0x...
PEER3_EVM_ADDRESS=0x...

# Contract addresses
TOKEN_NETWORK_REGISTRY=0xCbf6f43A17034e733744cBCc130FfcCA3CF3252C
M2M_TOKEN_ADDRESS=0x39eaF99Cd4965A28DFe8B1455DD42aB49D0836B9
```

**What's shared:**

- Ethereum addresses (for opening channels)
- Token contract address
- Registry contract address
- BTP authentication tokens

### Phase 2: Channel Opening (On-Chain)

**When:** First settlement threshold reached

**How:** One peer submits transaction to TokenNetworkRegistry

```solidity
// On-chain transaction (Base Sepolia)
tokenNetwork.openChannel(
  participant1,  // Connector A's address
  participant2,  // Connector B's address
  settleTimeout,
  initialDeposit
)
// Returns: channelId (bytes32)
```

**After this:** Both peers know the channelId from blockchain event

### Phase 3: Claim Exchange (Off-Chain via BTP)

**When:** Every settlement threshold

**How:** BTP `payment-channel-claim` sub-protocol (Epic 17)

```typescript
// Off-chain via WebSocket
const claim = {
  channelId: '0xabc...', // From Phase 2
  nonce: 42,
  transferredAmount: '5000000',
  signature: '0x...',
};

// Send via BTP
await btpClient.send({
  protocolName: 'payment-channel-claim',
  data: Buffer.from(JSON.stringify(claim)),
});
```

**This happens continuously** until channel is closed.

---

## Protocol Stack

```
┌─────────────────────────────────────────────────────────┐
│  Application Layer                                       │
│  - SPSP (end-user payment setup)                        │
│  - Payment pointers                                      │
└─────────────────────────────────────────────────────────┘
                        │
┌─────────────────────────────────────────────────────────┐
│  Transport Layer                                         │
│  - STREAM (RFC-0029)                                    │
│  - End-to-end payment streams                           │
└─────────────────────────────────────────────────────────┘
                        │
┌─────────────────────────────────────────────────────────┐
│  Interledger Layer                                       │
│  - ILPv4 packets (RFC-0027)                             │
│  - PREPARE / FULFILL / REJECT                           │
└─────────────────────────────────────────────────────────┘
                        │
┌─────────────────────────────────────────────────────────┐
│  Link Layer (Connector-to-Connector)                    │
│  - BTP (RFC-0023) - WebSocket transport                 │
│  - BTP sub-protocol: 'ilp' (ILP packets)               │
│  - BTP sub-protocol: 'payment-channel-claim' (Epic 17) │ ← HERE!
└─────────────────────────────────────────────────────────┘
                        │
┌─────────────────────────────────────────────────────────┐
│  Settlement Layer                                        │
│  - EVM payment channels (Ethereum/Base L2)              │
└─────────────────────────────────────────────────────────┘
```

---

## Why Not SPSP for Connectors?

**SPSP is designed for:**

- End-user wallet integration
- HTTPS-based (not WebSocket)
- One-time payment setup
- Human-readable payment pointers

**Connectors need:**

- Persistent WebSocket connections ✅ BTP
- Continuous claim exchange ✅ Epic 17
- Low-latency messaging ✅ BTP
- Bilateral settlement ✅ BTP

**BTP is the right protocol for connector-to-connector communication.**

---

## Summary

**Q: What protocol handles payment channel handshake?**

**A: Epic 17's BTP `payment-channel-claim` sub-protocol**

**NOT SPSP** - SPSP is for end-user payments, not connector settlement.

**The "handshake" consists of:**

1. Static config (peer addresses, contracts)
2. On-chain channel opening (when first needed)
3. Off-chain claim exchange via BTP (Epic 17)

**All communication happens over the existing BTP WebSocket connection!**

---

## Epic 31: Self-Describing Claims (SPSP Removal)

Epic 31 eliminates the need for out-of-band coordination (SPSP/Nostr kind:23194/23195) by embedding chain and contract coordinates directly in the claim message.

### Updated Three Phases

#### Phase 1 (Updated): Configuration from Public Records

**Before (SPSP):** Peers exchanged Nostr kind:23194 events containing settlement coordinates (chain, tokenNetworkAddress, tokenAddress, evmAddress).

**After (Epic 31):** Configuration is read from public Nostr kind:10032 records (replaceable events) or provided via Admin API. No bilateral exchange required — each connector publishes its own settlement capabilities.

#### Phase 2 (Updated): Unilateral Channel Opening

**Before:** Channel opening required both peers to have exchanged settlement coordinates via SPSP.

**After (Epic 31):** A connector opens a channel unilaterally using the peer's publicly available EVM address. The peer does not need to know about the channel in advance — it will discover it when the first self-describing claim arrives.

#### Phase 3 (Updated): Self-Describing Claim Exchange

**Before:** Claims referenced a `channelId` that both peers had pre-registered via SPSP handshake or Admin API.

**After (Epic 31):** Claims include optional self-describing fields:

```typescript
interface EVMClaimMessage {
  // ... existing fields (channelId, nonce, transferredAmount, etc.)

  // Epic 31 self-describing fields
  chainId?: number; // e.g., 8453 (Base) or 84532 (Base Sepolia)
  tokenNetworkAddress?: string; // TokenNetwork contract address
  tokenAddress?: string; // ERC20 token contract address
}
```

**Verification Flow for Unknown Channels:**

```
1. Peer sends BTP claim with self-describing fields
2. ClaimReceiver checks ChannelManager cache → miss (unknown channel)
3. ClaimReceiver extracts chainId, tokenNetworkAddress from claim
4. ClaimReceiver calls PaymentChannelSDK.getChannelState() → RPC read
5. Verify channel exists on-chain, sender is participant, state is 'opened'
6. Verify EIP-712 signature using domain from claim (not from config)
7. Auto-register channel in ChannelManager cache
8. Subsequent claims for same channel skip RPC (cached fast path)
```

**What This Eliminates:**

- Nostr kind:23194/23195 SPSP exchange
- Admin API `POST /admin/peers` for channel pre-registration
- Bilateral coordination before first settlement
- Static peer-to-contract address mapping in environment variables
