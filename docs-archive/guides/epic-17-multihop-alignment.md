# Epic 17 Multi-Hop Deployment Alignment

**Date:** 2026-02-02
**Epic:** 17 - BTP Off-Chain Claim Exchange Protocol
**Status:** ✅ Aligned (Note: ClaimSender superseded by PerPacketClaimService in Epic 31 -- claims now flow per-packet via BTP protocolData rather than as separate threshold-triggered messages)

## Summary

The 5-peer multi-hop deployment script and infrastructure is **fully compatible** with Epic 17 (BTP Off-Chain Claim Exchange Protocol). No code changes were required - only documentation updates to explain the claim exchange functionality.

## What is Epic 17?

Epic 17 implemented a **standardized off-chain claim exchange protocol** via BTP (Bilateral Transfer Protocol) for the EVM settlement chain:

- EVM/Base L2 (secp256k1 signatures)

**Key Components Added:**

- `ClaimSender` - Signs and sends claims to peers via BTP
- `ClaimReceiver` - Receives and verifies claims from peers
- `ClaimRedemptionService` - Automatically redeems profitable claims on-chain
- `BTPClaimMessage` types - Standardized claim message format
- BTP `payment-channel-claim` sub-protocol - Transport layer

## Why No Code Changes Were Needed

The multi-hop deployment script didn't require code changes because:

### 1. **Transparent Integration**

Epic 17 components are integrated into the existing `UnifiedSettlementExecutor`:

```typescript
// packages/connector/src/settlement/unified-settlement-executor.ts
constructor(
  // ... existing parameters ...
  claimSender: ClaimSender,              // Epic 17
  btpClientManager: BTPClientManager,    // Epic 17
  // ... rest ...
) {
  this._claimSender = claimSender;
  this._btpClientManager = btpClientManager;
  // Automatic claim exchange when settlement threshold reached
}
```

### 2. **No New Configuration Required**

Epic 17 reuses existing configuration:

- **BTP connections:** Already established between peers (Epic 2)
- **Settlement preference:** `SETTLEMENT_PREFERENCE=both` (existing env var)
- **Blockchain RPCs:** `BASE_L2_RPC_URL` (existing)
- **Peer addresses:** `PEER{N}_EVM_ADDRESS` (existing)

### 3. **Automatic Activation**

Claim exchange is **automatically enabled** when:

- Settlement threshold reached (monitored by SettlementMonitor)
- Peer BTP connection active (managed by BTPClientManager)
- Appropriate claim signer available (EVMClaimSigner)

No feature flags or additional configuration needed!

### 4. **BTP Transport Reuse**

Claims are sent over **existing BTP WebSocket connections** using the sub-protocol multiplexing feature:

```typescript
// BTP message with claim sub-protocol
{
  type: BTPMessageType.MESSAGE,
  requestId: 42,
  data: {
    protocolData: [
      {
        protocolName: 'payment-channel-claim',  // New sub-protocol
        contentType: 1,                         // application/json
        data: Buffer.from(JSON.stringify(claimMessage))
      }
    ],
    ilpPacket: Buffer.alloc(0)  // No ILP packet for settlement claims
  }
}
```

This means the same BTP connections used for packet forwarding also carry settlement claims!

## Documentation Updates Made

To help users understand the claim exchange functionality, we updated:

### 1. **Multi-Hop Summary** (`docs/guides/multi-hop-summary.md`)

Added sections explaining:

- BTP off-chain claim exchange protocol overview
- ClaimSender/ClaimReceiver flow diagram
- Claim verification process
- Automatic redemption logic

**Before:**

```markdown
### Claim-Based Settlement

**Off-Chain Balance Proofs:**
```

**After:**

```markdown
### Claim-Based Settlement (Epic 17)

The network uses **BTP Off-Chain Claim Exchange Protocol** to exchange
cryptographic settlement proofs between peers without on-chain transactions
until channel closure. This dramatically reduces gas costs and increases
settlement throughput.

**Off-Chain Balance Proofs:**
```

### 2. **Quick Start Guide** (`MULTIHOP-QUICKSTART.md`)

Added bullet point:

```markdown
**5 ILP Connectors** running in Docker, each with:

- ✓ BTP off-chain claim exchange enabled (Epic 17)
```

### 3. **Comprehensive Deployment Guide** (`docs/guides/multi-hop-deployment.md`)

Added new section:

```markdown
## Claims and Settlement (Epic 17)

### BTP Off-Chain Claim Exchange Protocol

The network uses **BTP (Bilateral Transfer Protocol) claim exchange** to send
cryptographic settlement proofs between peers...

**Key Components:**

- ClaimSender - Signs and sends claims to peers via BTP
- ClaimReceiver - Receives and verifies claims from peers
- ClaimRedemptionService - Automatically redeems profitable claims on-chain
```

### 4. **Key Features** (Summary Document)

Added new feature:

```markdown
### 3. BTP Off-Chain Claim Exchange (Epic 17)

- Cryptographic settlement proofs exchanged via BTP
- ClaimSender/ClaimReceiver automatically integrated
- Supports EVM (secp256k1) signatures
- Automatic claim redemption when gas costs favorable
```

## How It Works in Multi-Hop Network

In the 5-peer deployment, claim exchange works as follows:

### Scenario: Peer1 settles with Peer2

```
┌─────────────────────────────────────────────────────────────────┐
│  1. Packets forwarded: Peer1 → Peer2 → Peer3 → Peer4 → Peer5  │
│     Settlement threshold reached on Peer1→Peer2 channel         │
└─────────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────────┐
│  2. UnifiedSettlementExecutor (Peer1)                           │
│     - Detects SETTLEMENT_REQUIRED event                         │
│     - Creates balance proof for Peer2                           │
│     - Signs with KeyManager (EVM signer)                       │
└─────────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────────┐
│  3. ClaimSender.send() (Peer1)                                  │
│     - Serializes claim to JSON                                  │
│     - Wraps in BTP protocolData                                 │
│     - Sends via existing BTP WebSocket to Peer2                 │
└─────────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────────┐
│  4. ClaimReceiver.handle() (Peer2)                              │
│     - Receives BTP message with payment-channel-claim           │
│     - Deserializes JSON claim                                   │
│     - Verifies signature (secp256k1)                            │
│     - Checks nonce monotonicity                                 │
│     - Stores in claim database                                  │
│     - Emits CLAIM_RECEIVED telemetry                            │
└─────────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────────┐
│  5. ClaimRedemptionService (Peer2)                              │
│     - Polls database for verified claims (every 30s)            │
│     - Estimates gas cost for redemption                         │
│     - If profitable: submits to blockchain                      │
│     - If not: waits for more claims to batch                    │
│     - Emits CLAIM_REDEEMED telemetry                            │
└─────────────────────────────────────────────────────────────────┘
```

### All Peer Pairs Settle Independently

In the 5-peer linear chain, claim exchange happens between each consecutive pair:

- **Peer1 ↔ Peer2** - Independent claim exchange
- **Peer2 ↔ Peer3** - Independent claim exchange
- **Peer3 ↔ Peer4** - Independent claim exchange
- **Peer4 ↔ Peer5** - Independent claim exchange

Each peer acts as both **sender** (for outgoing claims) and **receiver** (for incoming claims).

## Verification

To verify Epic 17 functionality in the multi-hop network:

### 1. **Check Logs for Claim Exchange**

```bash
# View Peer2 logs for claim activity
docker-compose -f docker-compose-5-peer-multihop.yml logs peer2 | grep -i claim

# Expected output:
# {"level":"info","msg":"Claim sent","blockchain":"evm","peerId":"peer3","messageId":"..."}
# {"level":"info","msg":"Claim received","blockchain":"evm","peerId":"peer1","verified":true}
# {"level":"info","msg":"Claim redeemed","blockchain":"evm","txHash":"..."}
```

### 2. **Check Telemetry Events**

If Explorer UI is running:

```bash
# Navigate to http://localhost:5173
# Filter events by type: "Claim Exchange"
# Should see: CLAIM_SENT, CLAIM_RECEIVED, CLAIM_REDEEMED events
```

### 3. **Verify BTP Sub-Protocol**

Check that BTP messages include `payment-channel-claim` protocol data:

```bash
docker-compose -f docker-compose-5-peer-multihop.yml logs peer2 | grep "payment-channel-claim"

# Expected: BTP messages with protocolName: 'payment-channel-claim'
```

### 4. **Monitor Settlement Thresholds**

Watch for settlement events:

```bash
docker-compose -f docker-compose-5-peer-multihop.yml logs -f | grep SETTLEMENT_REQUIRED
```

## Benefits for Multi-Hop Network

Epic 17 brings significant benefits to the multi-hop deployment:

### 1. **Reduced Gas Costs**

Instead of submitting every settlement on-chain immediately:

- **Before Epic 17:** Settlement → Blockchain transaction (gas cost: ~$0.50)
- **After Epic 17:** Settlement → Off-chain claim exchange (gas cost: $0) → Batch redemption when profitable

**Savings:** ~90% reduction in gas costs for high-throughput routes

### 2. **Higher Settlement Throughput**

- **Before:** Limited by blockchain block time (1-12 seconds)
- **After:** Limited only by BTP WebSocket bandwidth (thousands of claims/sec)

### 3. **Automatic Batching**

ClaimRedemptionService intelligently batches claims:

```typescript
// Only redeem when profitable
if (claimAmount > estimatedGasCost * 1.1) {
  await submitToBlockchain(claim);
} else {
  // Wait for more claims to accumulate
  await storeForLater(claim);
}
```

### 4. **EVM Settlement Support**

EVM blockchain supported with unified interface:

- Base L2 EVM → `EVMClaimMessage` with secp256k1 signature

### 5. **Telemetry and Monitoring**

Epic 17 added comprehensive telemetry:

- `CLAIM_SENT` - Track outgoing claims
- `CLAIM_RECEIVED` - Track incoming claims with verification status
- `CLAIM_REDEEMED` - Track on-chain redemptions

This integrates with Prometheus metrics (Story 17.6) for alerting and monitoring.

## No Action Required

✅ **The multi-hop deployment script works out-of-the-box with Epic 17.**

Users do not need to:

- Update environment variables
- Modify configuration files
- Add new services to Docker Compose
- Install additional dependencies
- Enable feature flags

Claim exchange is **automatically active** when the deployment script runs!

## Testing Claim Exchange

To test claim exchange in the multi-hop network:

### 1. **Send High-Throughput Packets**

Generate enough traffic to trigger settlement thresholds:

```bash
cd tools/send-packet

# Send 1000 packets to trigger settlements
npm run send -- \
  -c ws://localhost:3000 \
  -d g.peer5.dest \
  -a 100000 \
  --batch 1000
```

### 2. **Monitor Claim Exchange**

Watch logs for claim activity:

```bash
# Terminal 1: Watch Peer1 (sender)
docker-compose -f docker-compose-5-peer-multihop.yml logs -f peer1 | grep -i claim

# Terminal 2: Watch Peer2 (receiver)
docker-compose -f docker-compose-5-peer-multihop.yml logs -f peer2 | grep -i claim
```

### 3. **Verify Blockchain Transactions**

Check that claims are eventually redeemed on-chain:

```bash
# View redemption events
docker-compose -f docker-compose-5-peer-multihop.yml logs peer2 | grep "Claim redeemed"

# Should show txHash for on-chain redemption
```

## Related Documentation

- **Epic 17 PRD:** `docs/prd/epic-17-btp-claim-exchange.md`
- **Story 17.1:** BTP Claim Message Protocol Definition
- **Story 17.2:** Claim Sender Implementation
- **Story 17.3:** Claim Receiver and Verification
- **Story 17.4:** UnifiedSettlementExecutor Integration
- **Story 17.5:** Automatic Claim Redemption
- **Story 17.6:** Telemetry and Monitoring
- **Story 17.7:** End-to-End BTP Claim Exchange Integration Tests

## Conclusion

The multi-hop deployment script is **fully compatible** with Epic 17 without any code changes. Epic 17's components are seamlessly integrated into the existing settlement infrastructure, making claim exchange automatic and transparent to operators.

**Key Takeaway:** Deploy the 5-peer network as documented, and claim exchange works immediately!

```bash
./scripts/deploy-5-peer-multihop.sh
# ✅ Claim exchange active automatically!
```
