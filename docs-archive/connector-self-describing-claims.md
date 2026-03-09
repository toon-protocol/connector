# Connector Handoff: Self-Describing BTP Claims

**From:** Crosstown protocol (Epic 3 Story 3.7)
**To:** `@crosstown/connector` (`/Users/jonathangreen/Documents/connector`)
**Date:** 2026-03-07
**Status:** Proposed

---

## Summary

The Crosstown protocol is removing the SPSP handshake (kind:23194/23195). Settlement details that were previously negotiated via encrypted Nostr events are now fully public in kind:10032 (ILP Peer Info). This means peers can open payment channels unilaterally and begin transacting without a prior handshake.

For this to work, the connector must handle **self-describing BTP claims** -- claims that carry their own chain and contract coordinates -- and verify payment channels **dynamically on-chain** on first contact, rather than requiring pre-registration via the Admin API.

---

## Why SPSP Is Being Removed

Crosstown uses TOON-over-ILP (not STREAM protocol). There is no shared secret to negotiate. Every field that SPSP negotiated is already available:

| SPSP field            | Where it already lives                                       |
| --------------------- | ------------------------------------------------------------ |
| `destinationAccount`  | kind:10032 `ilpAddress`                                      |
| `sharedSecret`        | Not used (no STREAM)                                         |
| `negotiatedChain`     | Deterministic from kind:10032 `supportedChains` intersection |
| `settlementAddress`   | kind:10032 `settlementAddresses`                             |
| `tokenAddress`        | kind:10032 `preferredTokens`                                 |
| `tokenNetworkAddress` | kind:10032 `tokenNetworks`                                   |
| `channelId`           | Opened unilaterally by sender                                |

The sender reads the peer's kind:10032 from the relay, selects the best matching chain locally (set intersection of `supportedChains`), opens a channel unilaterally on the TokenNetwork contract, and starts sending packets with self-describing claims.

---

## Change 1: Extend EVMClaimMessage

### Current format (`btp-claim-types.ts`)

```typescript
interface EVMClaimMessage {
  version: '1.0';
  blockchain: 'evm';
  messageId: string;
  timestamp: string;
  senderId: string;
  channelId: string; // bytes32
  nonce: number;
  transferredAmount: string;
  lockedAmount: string;
  locksRoot: string; // bytes32
  signature: string; // EIP-712
  signerAddress: string;
}
```

### New fields to add

```typescript
interface EVMClaimMessage {
  // ... all existing fields unchanged ...
  chainId: number; // e.g., 42161 (Arbitrum One), 31337 (Anvil)
  tokenNetworkAddress: string; // TokenNetwork contract address (0x-prefixed)
  tokenAddress: string; // ERC-20 token address (0x-prefixed)
}
```

These fields are already available in the connector's settlement config when sending claims. The `chainId` and `tokenNetworkAddress` are also already part of the EIP-712 signing domain (`{ name: 'TokenNetwork', version: '1', chainId, verifyingContract: tokenNetworkAddress }`), so they are cryptographically bound to the signature -- tampering with them causes signature verification to fail.

### Sending side (`claim-sender.ts`)

When `ClaimSender` builds a claim, populate the new fields from the existing `ChannelManager` metadata and settlement config. No new data sources needed -- the connector already knows these values for channels it manages.

---

## Change 2: Dynamic On-Chain Channel Verification

### Current behavior

The connector requires channels to be pre-registered via Admin API (`POST /admin/channels`) or `ChannelManager.ensureChannelExists()` before claims can be received. `ClaimReceiver` verifies the EIP-712 signature but relies on the `ChannelManager.channelMetadata` map to know the channel is valid.

### New behavior

When `ClaimReceiver` receives a claim referencing a channel NOT in `channelMetadata`:

1. **Extract contract coordinates** from the claim: `chainId`, `tokenNetworkAddress`, `channelId`
2. **Query on-chain**: call `channels(channelId)` on the `tokenNetworkAddress` contract at `chainId`
3. **Verify**:
   - Channel exists and is in `open` state (state === 1)
   - `signerAddress` from the claim matches `participant1` or `participant2` on-chain
4. **Verify EIP-712 signature** using the domain reconstructed from claim fields: `{ name: 'TokenNetwork', version: '1', chainId: claim.chainId, verifyingContract: claim.tokenNetworkAddress }`
5. **Register in ChannelManager**: add the verified channel to `channelMetadata` map and `peerChannelIndex`
6. **Process normally**: subsequent claims for this channel skip the RPC call (signature + nonce check only, same as today)

### Key property: TokenNetwork enforces one channel per participant pair

The TokenNetwork contract prevents multiple open channels between the same two addresses. This means given two addresses, there is at most one active channel. The `channelId` in the claim is sufficient to uniquely identify it, and on-chain lookup is deterministic.

---

## Change 3: Auto-Register Peer on First Verified Claim

When a claim from an unknown peer is successfully verified on-chain, the connector should auto-register that peer as routable. Currently, peers must be explicitly registered via Admin API before packets can be routed to/from them.

The BTP WebSocket connection already exists (the peer connected to send the claim). The connector should:

1. Associate the BTP connection with a `peerId` derived from the claim's `senderId`
2. Add the peer to the routing table
3. Allow the connector to route packets back to this peer over the same BTP connection

This enables the FULFILL/REJECT response to flow back without any prior peer setup.

---

## Change 4: Channel Close Event Watching (Nice-to-Have)

Subscribe to `ChannelClosed` events on TokenNetwork contracts to invalidate cached channel entries. When a channel is closed on-chain:

1. Remove the channel from `channelMetadata`
2. Remove from `peerChannelIndex`
3. Subsequent claims for that channel trigger fresh on-chain verification (which will fail since the channel is no longer `open`)

This is a safety measure. Nonce monotonicity + EIP-712 signature verification already prevent replay attacks. A closed channel's claims would fail nonce checks if the channel was re-opened with a new nonce sequence.

---

## Flow: First Contact from Unknown Peer

```
Bob (new peer)                    Alice's Connector
 │                                      │
 │  1. Connect to btpEndpoint           │
 │  ─────── WS connect ──────────────> │
 │                                      │
 │  2. BTP message:                     │
 │     protocolData: claim {            │
 │       channelId, chainId,            │
 │       tokenNetworkAddress,           │
 │       tokenAddress, nonce,           │
 │       transferredAmount, signature,  │
 │       signerAddress, senderId        │
 │     }                                │
 │     packet: ILP PREPARE              │
 │  ─────── BTP request ─────────────> │
 │                                      │  3. ClaimReceiver:
 │                                      │     channelId NOT in channelMetadata
 │                                      │     → RPC: channels(channelId) on
 │                                      │       tokenNetworkAddress @ chainId
 │                                      │     → Verify: open, signerAddress
 │                                      │       is participant
 │                                      │     → Verify EIP-712 signature
 │                                      │     → Add to channelMetadata (cached)
 │                                      │     → Auto-register peer
 │                                      │
 │                                      │  4. Route ILP PREPARE to BLS
 │                                      │
 │  5. BTP response (FULFILL)           │
 │  <──── same WS connection ───────── │
 │                                      │
 │  Subsequent packets: signature +     │
 │  nonce check only (no RPC)           │
```

---

## Files to Modify

| File                                                       | Change                                                                                                        |
| ---------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------- |
| `packages/connector/src/btp/btp-claim-types.ts`            | Add `chainId`, `tokenNetworkAddress`, `tokenAddress` to `EVMClaimMessage`, update validation                  |
| `packages/connector/src/settlement/claim-sender.ts`        | Populate new fields from channel metadata when building claims                                                |
| `packages/connector/src/settlement/claim-receiver.ts`      | Add dynamic on-chain verification path for unknown channels                                                   |
| `packages/connector/src/settlement/payment-channel-sdk.ts` | May need method to verify a channel by ID on a given TokenNetwork (already has `channels()` query capability) |
| `packages/connector/src/settlement/channel-manager.ts`     | Support adding channels discovered via incoming claims (not just Admin API)                                   |
| `packages/connector/src/http/admin-api.ts`                 | Channel pre-registration via Admin API remains supported but is no longer required                            |

---

## Backward Compatibility

- Claims WITHOUT the new fields should still be accepted if the channel is already in `channelMetadata` (pre-registered via Admin API). This allows gradual migration.
- Claims WITH the new fields from unknown channels trigger the dynamic verification path.
- The Admin API channel registration endpoints remain functional for manual/legacy use.

---

## Testing

- **Unit**: Extended claim validation (new fields present, correct format)
- **Unit**: Dynamic verification path (mock RPC responses for channel lookup)
- **Unit**: Auto-peer-registration on first verified claim
- **Integration**: End-to-end flow -- unknown peer sends claim, connector verifies on-chain, processes packet, returns FULFILL
- **Integration**: Subsequent claims from same channel skip RPC (cached)
- **Integration**: Claim with tampered chainId/tokenNetworkAddress fails signature verification
