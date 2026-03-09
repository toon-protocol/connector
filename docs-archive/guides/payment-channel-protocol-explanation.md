# Payment Channel Setup Protocol in ILP

## Your Question: Is it SPSP?

**No, SPSP is NOT used for payment channel setup between connectors.**

## What is SPSP?

**SPSP (Simple Payment Setup Protocol - RFC-0009)** is for:

- End-user payment setup (sender → receiver)
- Resolving payment pointers ($alice@example.com)
- Getting receiver information via HTTPS
- Setting up STREAM connections

**NOT for connector-to-connector channel setup.**

## What M2M Uses for Payment Channels

### 1. Configuration-Based Setup (Static)

Payment channel parameters are configured **out-of-band** via:

**A. YAML Configuration Files:**

```yaml
peers:
  - id: peer2
    url: ws://peer2:3001
    authToken: secret-peer1-to-peer2
    # Channel params configured separately
```

**B. Environment Variables:**

```env
PEER2_EVM_ADDRESS=0x...
TOKEN_NETWORK_REGISTRY=0xCbf6f43A17034e733744cBCc130FfcCA3CF3252C
M2M_TOKEN_ADDRESS=0x39eaF99Cd4965A28DFe8B1455DD42aB49D0836B9
```

### 2. BTP for Claim Exchange (Dynamic)

**BTP (Bilateral Transfer Protocol - RFC-0023)** is used for:

- WebSocket connections between connectors
- ILP packet forwarding
- **Epic 17: Off-chain claim exchange** ✅

**BTP Sub-Protocols:**

```typescript
// BTP message structure
{
  type: MESSAGE,
  protocolData: [
    {
      protocolName: 'ilp',  // ILP packets
      data: <serialized ILP packet>
    },
    {
      protocolName: 'payment-channel-claim',  // Epic 17
      data: <claim message>
    }
  ]
}
```

### 3. Epic 17: BTP Off-Chain Claim Exchange

**This is what shares payment channel information:**

```typescript
// Claim message sent via BTP
{
  blockchain: 'evm',
  channelId: '0xabc...',
  nonce: 42,
  transferredAmount: '5000000',
  signature: '0x...',
  publicKey: '0x...'
}
```

**Flow:**

1. Connector A reaches settlement threshold
2. Signs balance proof (claim)
3. Sends claim to Connector B via BTP `payment-channel-claim` sub-protocol
4. Connector B verifies signature
5. Stores claim for later on-chain redemption

**This is the "handshake" for payment channels in M2M!**

## Protocol Comparison

| Protocol    | Purpose                 | Used For                                       |
| ----------- | ----------------------- | ---------------------------------------------- |
| **SPSP**    | End-user payment setup  | Sender → Receiver (not connector-to-connector) |
| **BTP**     | Connector communication | ILP packets + claim exchange                   |
| **STREAM**  | Transport layer         | End-to-end payment streams                     |
| **Epic 17** | Settlement claims       | Payment channel balance proofs                 |

## How Payment Channels Work in M2M

### Initial Setup (Static)

1. **Deploy contracts** - TokenNetworkRegistry on Base Sepolia
2. **Configure peers** - Map peer IDs to Ethereum addresses
3. **Fund wallets** - Each peer needs ETH + M2M tokens

### Channel Opening (On-Demand)

When settlement threshold reached:

```typescript
// UnifiedSettlementExecutor
async handleSettlement(event: SettlementRequiredEvent) {
  // 1. Ensure channel exists (opens if needed)
  const channelId = await channelManager.ensureChannelExists(peerId, tokenId);

  // 2. Sign balance proof
  const claim = await claimSigner.signClaim(channelId, amount);

  // 3. Send via BTP (Epic 17)
  await claimSender.send(peerId, claim);
}
```

### Claim Exchange (Dynamic via BTP)

**This is the "handshake":**

```
Connector A                    Connector B
───────────                    ───────────
Settlement threshold reached
     ↓
Sign claim (off-chain)
     ↓
BTP WebSocket
  protocolName: 'payment-channel-claim'
  data: {channelId, nonce, amount, signature}
     ↓                        ───────────▶
                              Receive claim via BTP
                                   ↓
                              Verify signature
                                   ↓
                              Store in database
                                   ↓
                              Later: Redeem on-chain
```

## Answer Summary

**Payment channel info is shared via:**

1. **Static config** - Peer addresses, token addresses (out-of-band)
2. **BTP `payment-channel-claim` sub-protocol** - Balance proofs (Epic 17)
3. **NOT SPSP** - SPSP is for end-user payments, not connector settlement

**The "handshake" is Epic 17's BTP claim exchange protocol!**
