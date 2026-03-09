# Epic 31: Self-Describing BTP Claims & Dynamic Channel Verification

**Status:** Proposed
**Origin:** [Connector Self-Describing Claims Handoff](../connector-self-describing-claims.md) (Crosstown Epic 3, Story 3.7)
**Date:** 2026-03-07

---

## Epic Goal

Enable the connector to accept claims from unknown peers by extending BTP claims with chain/contract coordinates and verifying payment channels dynamically on-chain, eliminating the requirement for Admin API channel pre-registration and enabling unilateral channel opening.

## Epic Description

### Existing System Context

- **Current functionality:** Epic 17 established the BTP claim exchange protocol (`ClaimSender`, `ClaimReceiver`, `ChannelManager`). Claims carry EIP-712 signed balance proofs over BTP WebSocket. Channels must be pre-registered via Admin API (`POST /admin/channels`) or `ChannelManager.ensureChannelExists()` before claims can be received.
- **Technology stack:** TypeScript 5.3.3, Node.js 22+, ethers ^6.16.0, ws ^8.16.0, Jest 29.7.x, EVM-only settlement (Epic 30 removed XRP/Aptos)
- **Integration points:** `btp-claim-types.ts` (message schema), `claim-sender.ts` / `claim-receiver.ts` (send/verify), `channel-manager.ts` (channel lifecycle), `payment-channel-sdk.ts` (on-chain queries)

### Enhancement Details

- **What's changing:** BTP claims gain three new fields (`chainId`, `tokenNetworkAddress`, `tokenAddress`) making them self-describing. The receiver can verify unknown channels on-chain and auto-register peers, enabling unilateral channel opening without prior handshake.
- **Why:** Crosstown is removing the SPSP handshake (kind:23194/23195). All settlement details are now public in kind:10032 (ILP Peer Info). Peers must be able to open channels unilaterally and start transacting without connector-side pre-registration.
- **How it integrates:** Extends existing `EVMClaimMessage` interface, adds a dynamic verification branch in `ClaimReceiver`, and extends `ChannelManager` to accept externally-discovered channels.

### Success Criteria

- Unknown peer can send a self-describing claim and receive a FULFILL response without any prior Admin API registration
- Subsequent claims from the same channel skip RPC (cached in `channelMetadata`)
- Claims with tampered `chainId`/`tokenNetworkAddress` fail EIP-712 signature verification
- Pre-registered channels (Admin API) continue working with or without new fields (backward compat)

---

## Stories

### Story 31.1: Extend EVMClaimMessage with Self-Describing Fields

**Goal:** Add chain and contract coordinates to BTP claims so they are self-describing.

**Changes:**

| File                                                | Change                                                                                                                                                                                                                                              |
| --------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `packages/connector/src/btp/btp-claim-types.ts`     | Add `chainId` (number), `tokenNetworkAddress` (string), `tokenAddress` (string) to `EVMClaimMessage`. Update `validateEVMClaim()` to validate new fields when present. Fields are optional for backward compatibility with pre-registered channels. |
| `packages/connector/src/settlement/claim-sender.ts` | Update `sendEVMClaim()` to populate new fields from `ChannelManager` metadata and settlement config. No new data sources needed -- the connector already knows these values for channels it manages.                                                |

**Acceptance Criteria:**

- `EVMClaimMessage` interface includes `chainId?`, `tokenNetworkAddress?`, `tokenAddress?`
- `validateEVMClaim()` validates new fields when present (correct types, 0x-prefix for addresses)
- `validateEVMClaim()` passes when new fields are absent (backward compat)
- `ClaimSender` populates new fields from existing channel metadata
- Unit tests cover validation with/without new fields, correct/incorrect formats

---

### Story 31.2: Dynamic On-Chain Channel Verification & Auto-Peer Registration

**Goal:** Enable the connector to verify unknown channels on-chain and auto-register peers on first contact.

**Changes:**

| File                                                       | Change                                                                                                                                                                                                                                                                                                                                              |
| ---------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `packages/connector/src/settlement/claim-receiver.ts`      | Add dynamic verification path: when a claim references a channel NOT in `channelMetadata`, extract `chainId`/`tokenNetworkAddress`/`channelId` from claim, query on-chain via `PaymentChannelSDK`, verify channel is open and `signerAddress` is a participant, verify EIP-712 signature using domain from claim fields, register channel and peer. |
| `packages/connector/src/settlement/channel-manager.ts`     | Add method to register externally-discovered channels (populate `channelMetadata` and `peerChannelIndex` from claim data).                                                                                                                                                                                                                          |
| `packages/connector/src/settlement/payment-channel-sdk.ts` | Ensure `channels(channelId)` query is available for arbitrary `tokenNetworkAddress`/`chainId` (may need to accept contract address as parameter rather than using only the pre-configured one).                                                                                                                                                     |

**Verification Flow (first contact):**

1. `ClaimReceiver` receives claim with `channelId` NOT in `channelMetadata`
2. Extract contract coordinates: `chainId`, `tokenNetworkAddress`, `channelId`
3. RPC call: `channels(channelId)` on `tokenNetworkAddress` at `chainId`
4. Verify: channel exists, state === open (1), `signerAddress` matches `participant1` or `participant2`
5. Verify EIP-712 signature with domain: `{ name: 'TokenNetwork', version: '1', chainId: claim.chainId, verifyingContract: claim.tokenNetworkAddress }`
6. Register channel in `ChannelManager` (add to `channelMetadata` and `peerChannelIndex`)
7. Auto-register peer: associate BTP connection with `peerId` from `senderId`, add to routing table
8. Process normally -- subsequent claims skip RPC

**Acceptance Criteria:**

- Claims for unknown channels with valid on-chain state are accepted
- Claims for non-existent or closed channels are rejected with clear error
- Claims where `signerAddress` is not a channel participant are rejected
- Verified channels are cached -- second claim skips RPC
- Peer is auto-registered and routable after first verified claim
- Unit tests with mocked RPC responses cover all verification branches

---

### Story 31.3: Integration Tests & Backward Compatibility Verification

**Goal:** End-to-end validation of the self-describing claims flow and backward compatibility.

**Test Scenarios:**

1. **First contact flow:** Unknown peer connects via BTP, sends self-describing claim, connector verifies on-chain, processes ILP PREPARE, returns FULFILL over same WebSocket
2. **Caching:** Subsequent claims from the same channel skip RPC (verify no additional RPC calls)
3. **Tampered fields:** Claim with modified `chainId` or `tokenNetworkAddress` fails EIP-712 signature verification
4. **Backward compat:** Pre-registered channel (via Admin API) continues to work with claims that omit new fields
5. **Mixed mode:** Pre-registered channel works with claims that include new fields (fields are informational, channel already cached)

**Infrastructure:**

- Anvil-based integration tests following Epic 30 test infrastructure patterns
- TokenNetwork contract deployed to local Anvil for on-chain verification
- Mock BTP WebSocket connections for peer simulation

**Acceptance Criteria:**

- All 5 test scenarios pass
- No regression in existing claim exchange tests
- Tests run in CI without external dependencies (Anvil only)

---

## Deferred: Channel Close Event Watching

Change 4 from the handoff doc (subscribing to `ChannelClosed` events to invalidate cached channels) is deferred to a follow-up enhancement. Nonce monotonicity + EIP-712 verification already prevent replay attacks. A closed channel's claims would fail nonce checks if the channel were re-opened with a new nonce sequence.

---

## Compatibility Requirements

- [x] Claims WITHOUT new fields accepted for pre-registered channels (backward compat)
- [x] Admin API channel registration endpoints remain functional
- [x] Existing `ChannelManagerConfig` and YAML configs work unchanged
- [x] No changes to ILP packet format or routing protocol
- [x] EIP-712 domain binding ensures cryptographic integrity of new fields

## Risk Mitigation

- **Primary Risk:** RPC latency on first contact could delay initial packet processing
- **Mitigation:** On-chain verification only on first claim per channel; subsequent claims are cached. The RPC call is a single `channels(channelId)` read -- fast even on public endpoints.
- **Rollback Plan:** New fields are optional. If issues arise, revert to pre-registration-only mode by removing the dynamic verification branch. No schema migrations to undo.

## Definition of Done

- [ ] All 3 stories completed with acceptance criteria met
- [ ] Existing claim exchange functionality verified (no regression)
- [ ] Pre-registered channel flow still works
- [ ] Dynamic verification flow works end-to-end
- [ ] Integration tests pass against Anvil
- [ ] No regression in existing features
- [ ] Documentation updated appropriately
