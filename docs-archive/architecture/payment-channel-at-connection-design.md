# Design: Payment Channel Creation at Peer Connection

**Author:** Claude Code
**Date:** 2026-02-03
**Status:** Design Proposal
**Epic:** Future Enhancement

## Problem Statement

**Current Behavior (Incorrect):**
Payment channels are created **on-demand** when settlement thresholds are reached during packet forwarding.

**Desired Behavior:**
Payment channels should be created **immediately** when peer BTP connections are established.

**Why This Matters:**

- Channels should be ready before forwarding any packets
- Avoids race conditions during first settlement
- Clearer separation of concerns (connection setup vs settlement)
- More predictable system behavior
- Easier to reason about system state

## Current Architecture

### How It Works Now

```
┌──────────────────────────────────────────────────────────────┐
│ ConnectorNode Startup                                        │
├──────────────────────────────────────────────────────────────┤
│ 1. Load configuration from YAML                              │
│ 2. Initialize BTPClientManager                               │
│ 3. Connect to all configured peers (BTP WebSocket)           │
│ 4. Start forwarding packets                                  │
│    └─> Settlement is separate, channels created later        │
└──────────────────────────────────────────────────────────────┘
                          │
                          ▼
┌──────────────────────────────────────────────────────────────┐
│ Packet Forwarding (PacketHandler)                           │
├──────────────────────────────────────────────────────────────┤
│ 1. Receive PREPARE packet                                    │
│ 2. Route to next-hop peer                                    │
│ 3. Forward via BTP                                           │
│ 4. Record settlement (if enabled)                            │
│    └─> Accumulate balance in SettlementMonitor              │
└──────────────────────────────────────────────────────────────┘
                          │
                          ▼ (After threshold reached)
┌──────────────────────────────────────────────────────────────┐
│ UnifiedSettlementExecutor                                    │
├──────────────────────────────────────────────────────────────┤
│ 1. SETTLEMENT_REQUIRED event triggered                       │
│ 2. ensureChannelExists(peerId, tokenId)  ← CHANNEL CREATED  │
│ 3. Sign balance proof (claim)                                │
│ 4. Send claim via BTP (Epic 17)                              │
└──────────────────────────────────────────────────────────────┘
```

**Problem:** Channel creation happens **after** packets are forwarded, not **before**.

### Components Involved

**ConnectorNode** (`src/core/connector-node.ts`)

- Initializes BTP connections
- Does NOT initialize settlement stack
- No access to ChannelManager

**ChannelManager** (`src/settlement/channel-manager.ts`)

- Has `ensureChannelExists()` method
- Manages channel lifecycle
- NOT instantiated in ConnectorNode

**UnifiedSettlementExecutor** (`src/settlement/unified-settlement-executor.ts`)

- Receives SETTLEMENT_REQUIRED events
- Creates channels on-demand
- Separate from ConnectorNode

## Proposed Architecture

### How It Should Work

```
┌──────────────────────────────────────────────────────────────┐
│ ConnectorNode Startup                                        │
├──────────────────────────────────────────────────────────────┤
│ 1. Load configuration from YAML                              │
│ 2. Initialize Settlement Stack                               │
│    ├─> PaymentChannelSDK                                     │
│    ├─> ChannelManager                                        │
│    └─> SettlementExecutor                                    │
│ 3. Initialize BTPClientManager                               │
│ 4. Connect to all configured peers (BTP WebSocket)           │
│ 5. ✨ CREATE PAYMENT CHANNELS FOR CONNECTED PEERS            │
│    └─> channelManager.ensureChannelExists(peerId, tokenId)  │
│ 6. Start forwarding packets                                  │
│    └─> Channels already exist and ready                     │
└──────────────────────────────────────────────────────────────┘
                          │
                          ▼
┌──────────────────────────────────────────────────────────────┐
│ Packet Forwarding (PacketHandler)                           │
├──────────────────────────────────────────────────────────────┤
│ 1. Receive PREPARE packet                                    │
│ 2. Route to next-hop peer                                    │
│ 3. Forward via BTP                                           │
│ 4. Record settlement (channels ALREADY EXIST)                │
└──────────────────────────────────────────────────────────────┘
                          │
                          ▼ (When threshold reached)
┌──────────────────────────────────────────────────────────────┐
│ UnifiedSettlementExecutor                                    │
├──────────────────────────────────────────────────────────────┤
│ 1. SETTLEMENT_REQUIRED event triggered                       │
│ 2. Channel already exists (created at startup)               │
│ 3. Sign balance proof (claim)                                │
│ 4. Send claim via BTP (Epic 17)                              │
└──────────────────────────────────────────────────────────────┘
```

**Benefit:** Channels exist **before** any packets are forwarded.

## Implementation Plan

### Phase 1: Add Settlement Stack to ConnectorNode

**File:** `packages/connector/src/core/connector-node.ts`

**Add Members:**

```typescript
export class ConnectorNode implements HealthStatusProvider {
  // Existing members...
  private readonly _telemetryEmitter: TelemetryEmitter | null;

  // NEW: Settlement stack
  private _paymentChannelSDK: PaymentChannelSDK | null = null;
  private _channelManager: ChannelManager | null = null;
  private _settlementExecutor: SettlementExecutor | null = null;
  private _settlementMonitor: SettlementMonitor | null = null;
  private _accountManager: AccountManager | null = null;
```

**Add Initialization:**

```typescript
constructor(configFilePath: string, logger: Logger) {
  // ... existing initialization ...

  // Initialize settlement stack if enabled
  if (config.settlement?.enableSettlement) {
    this.initializeSettlementStack(config);
  }
}

private initializeSettlementStack(config: ConnectorConfig): void {
  // 1. Create Ethereum provider
  const provider = new ethers.JsonRpcProvider(process.env.BASE_L2_RPC_URL);
  const wallet = new ethers.Wallet(process.env.TREASURY_EVM_PRIVATE_KEY!, provider);

  // 2. Initialize PaymentChannelSDK
  this._paymentChannelSDK = new PaymentChannelSDK(
    wallet,
    process.env.TOKEN_NETWORK_REGISTRY!,
    this._logger
  );

  // 3. Build peer-to-address mapping
  const peerIdToAddressMap = new Map<string, string>();
  // Load from environment: PEER1_EVM_ADDRESS, PEER2_EVM_ADDRESS, etc.

  // 4. Initialize ChannelManager
  this._channelManager = new ChannelManager(
    {
      nodeId: config.nodeId,
      defaultSettlementTimeout: 86400,
      initialDepositMultiplier: config.settlement?.initialDepositMultiplier ?? 10,
      idleChannelThreshold: 86400,
      minDepositThreshold: 0.5,
      idleCheckInterval: 3600,
      tokenAddressMap: new Map([['M2M', process.env.M2M_TOKEN_ADDRESS!]]),
      peerIdToAddressMap,
      registryAddress: process.env.TOKEN_NETWORK_REGISTRY!,
      rpcUrl: process.env.BASE_L2_RPC_URL!,
      privateKey: process.env.TREASURY_EVM_PRIVATE_KEY!
    },
    this._paymentChannelSDK,
    // ... other dependencies
    this._logger
  );

  // 5. Start ChannelManager
  this._channelManager.start();
}
```

### Phase 2: Create Channels After Peer Connections

**File:** `packages/connector/src/core/connector-node.ts`

**Location:** After line 324 in `start()` method

```typescript
async start(): Promise<void> {
  // ... existing peer connection code ...

  // Wait for all peer connection attempts
  const peerResults = await Promise.allSettled(peerConnections);

  // Get connected peers
  const connectedPeers = this._btpClientManager.getPeerStatus();

  // NEW: Create payment channels for all connected peers
  if (this._channelManager && this._config.settlement?.enableSettlement) {
    await this.createPaymentChannels(connectedPeers);
  }

  // ... rest of startup ...
}

private async createPaymentChannels(
  connectedPeers: Map<string, boolean>
): Promise<void> {
  this._logger.info(
    { peerCount: connectedPeers.size },
    'Creating payment channels for connected peers'
  );

  const channelCreations: Promise<void>[] = [];

  for (const [peerId, connected] of connectedPeers.entries()) {
    if (!connected) {
      this._logger.warn({ peerId }, 'Skipping channel creation for disconnected peer');
      continue;
    }

    // Create channel creation task
    const createChannel = async (): Promise<void> => {
      try {
        // Determine token ID (from config or default)
        const tokenId = this.getTokenIdForPeer(peerId) ?? 'M2M';

        // Create or verify channel exists
        const channelId = await this._channelManager!.ensureChannelExists(peerId, tokenId);

        this._logger.info(
          { peerId, channelId, tokenId },
          'Payment channel ready'
        );
      } catch (error) {
        // Don't fail startup if channel creation fails
        // Settlement will retry on-demand later
        const errorMessage = error instanceof Error ? error.message : String(error);
        this._logger.warn(
          { peerId, error: errorMessage },
          'Failed to create payment channel (will retry on settlement)'
        );
      }
    };

    channelCreations.push(createChannel());
  }

  // Create all channels in parallel
  await Promise.allSettled(channelCreations);

  this._logger.info('Payment channel creation complete');
}

private getTokenIdForPeer(peerId: string): string | null {
  // TODO: Add token configuration to peer config
  // For now, default to M2M token
  return 'M2M';
}
```

### Phase 3: Update Configuration Types

**File:** `packages/connector/src/config/types.ts`

**Extend PeerConfig:**

```typescript
export interface PeerConfig {
  id: string;
  url: string;
  authToken: string;

  // NEW: Payment channel configuration
  evmAddress?: string; // Ethereum address for EVM channels
  preferredTokens?: string[]; // Preferred settlement tokens
}
```

**Update YAML Schema:**

```yaml
peers:
  - id: peer2
    url: ws://peer2:3001
    authToken: secret-peer1-to-peer2
    evmAddress: 0x... # NEW
    preferredTokens: ['M2M'] # NEW
```

### Phase 4: Handle Startup Failures

**Considerations:**

1. **Slow Startup**
   - On-chain channel creation takes 2-10 seconds per channel
   - With 4 channels (peer1↔peer2, peer2↔peer3, peer3↔peer4, peer4↔peer5), could take 40 seconds
   - Can create in parallel to reduce to ~10 seconds

2. **Partial Failures**
   - What if channel creation fails for some peers?
   - Should connector start anyway (allow packet routing without settlement)?
   - Or fail fast and prevent startup?

3. **Configuration Validation**
   - Need all peer Ethereum addresses configured upfront
   - Need TOKEN_NETWORK_REGISTRY address
   - Need funded wallets with ETH + tokens

**Recommended Approach:**

- Create channels in parallel (faster)
- Log warnings for failures but continue startup
- Allow packet routing even if channels fail
- Settlement will retry channel creation on-demand

### Phase 5: Update Health Checks

**Add channel health to health endpoint:**

```typescript
getHealthStatus(): HealthStatus {
  const channelStatus = this._channelManager?.getAllChannels() ?? [];

  return {
    status: this._healthStatus,
    uptime: Date.now() - this._startTime.getTime(),
    peersConnected,
    totalPeers,
    // NEW: Channel information
    channelsOpen: channelStatus.filter(c => c.status === 'active').length,
    totalChannels: channelStatus.length,
    timestamp: new Date().toISOString(),
    nodeId: this._config.nodeId,
    version: packageJson.version
  };
}
```

## Configuration Requirements

### Environment Variables Needed

```env
# Settlement Configuration
SETTLEMENT_ENABLED=true
SETTLEMENT_THRESHOLD=1000000
INITIAL_DEPOSIT_MULTIPLIER=10

# EVM Settlement (Base Sepolia)
BASE_L2_RPC_URL=https://sepolia.base.org
TOKEN_NETWORK_REGISTRY=0xCbf6f43A17034e733744cBCc130FfcCA3CF3252C
M2M_TOKEN_ADDRESS=0x39eaF99Cd4965A28DFe8B1455DD42aB49D0836B9

# Connector Wallet (needs ETH for gas + M2M tokens for deposits)
TREASURY_EVM_PRIVATE_KEY=0x...

# Peer Addresses (for channel opening)
PEER1_EVM_ADDRESS=0x...
PEER2_EVM_ADDRESS=0x...
PEER3_EVM_ADDRESS=0x...
PEER4_EVM_ADDRESS=0x...
PEER5_EVM_ADDRESS=0x...
```

### YAML Configuration

```yaml
# examples/multihop-peer2.yaml
nodeId: peer2
ilpAddress: g.peer2
btpServerPort: 3001

# Peer connections with settlement addresses
peers:
  - id: peer1
    url: ws://peer1:3000
    authToken: secret-peer1-to-peer2
    evmAddress: ${PEER1_EVM_ADDRESS}
    preferredTokens: ['M2M']

  - id: peer3
    url: ws://peer3:3002
    authToken: secret-peer2-to-peer3
    evmAddress: ${PEER3_EVM_ADDRESS}
    preferredTokens: ['M2M']

# Settlement configuration
settlement:
  enableSettlement: true
  settlementThreshold: 1000000
  connectorFeePercentage: 0.1
  initialDepositMultiplier: 10
```

## Implementation Steps

### Step 1: Integrate Settlement Stack into ConnectorNode

**Files to modify:**

- `packages/connector/src/core/connector-node.ts`

**Dependencies to add:**

- PaymentChannelSDK
- ChannelManager
- SettlementMonitor
- SettlementExecutor
- AccountManager (TigerBeetle)

**Estimated effort:** 4-6 hours

### Step 2: Add Channel Creation After Peer Connections

**Location:** `connector-node.ts` line ~324

**New method:** `createPaymentChannels()`

**Estimated effort:** 2-3 hours

### Step 3: Update Configuration Schema

**Files to modify:**

- `packages/connector/src/config/types.ts` - Add peer settlement config
- All peer YAML files - Add evmAddress field

**Estimated effort:** 1-2 hours

### Step 4: Handle Error Cases

**Scenarios to handle:**

- Peer has no Ethereum address configured
- Insufficient funds for channel deposit
- On-chain transaction fails
- Network connectivity issues

**Error handling strategy:**

- Log warning for channel creation failures
- Continue connector startup
- Allow packet routing without settlement
- Retry channel creation on first settlement attempt

**Estimated effort:** 2-3 hours

### Step 5: Update Health Checks and Telemetry

**Add to health endpoint:**

- Channel count (open, closing, closed)
- Channel details per peer

**Add telemetry events:**

- CHANNEL_CREATION_STARTED
- CHANNEL_CREATION_SUCCESS
- CHANNEL_CREATION_FAILED

**Estimated effort:** 1-2 hours

### Step 6: Testing

**Test cases:**

1. Channels created successfully for all peers
2. Partial channel creation (some succeed, some fail)
3. No funds available for deposits
4. Settlement works with pre-created channels
5. Channel opening transaction on Base Sepolia
6. Verify channels in Explorer UI

**Estimated effort:** 3-4 hours

## Total Estimated Effort

**Implementation:** 13-20 hours
**Testing:** 3-4 hours
**Documentation:** 2-3 hours
**Total:** 18-27 hours (2-3 days of development)

## Benefits of This Approach

1. **Predictable State**
   - Channels exist before packet forwarding starts
   - No race conditions during first settlement
   - Clear system initialization sequence

2. **Better Error Handling**
   - Channel creation failures visible at startup
   - Easier to diagnose configuration issues
   - Can validate channel setup before accepting traffic

3. **Separation of Concerns**
   - Connection setup: BTP WebSocket + payment channels
   - Packet forwarding: Routing + settlement recording
   - Settlement execution: Claim signing + BTP transmission

4. **Performance**
   - First settlement is faster (channel already exists)
   - No on-chain delay during packet processing
   - Parallel channel creation at startup

## Risks and Mitigations

### Risk 1: Slow Startup

**Problem:** Creating 4 channels takes 10-40 seconds

**Mitigation:**

- Create channels in parallel (reduce to ~10 seconds)
- Make channel creation optional (failsafe mode)
- Add startup progress logging

### Risk 2: Startup Failures

**Problem:** Channel creation might fail, preventing connector start

**Mitigation:**

- Don't fail startup on channel errors
- Log warnings and continue
- Allow packet routing without settlement
- Retry on first settlement attempt

### Risk 3: Configuration Complexity

**Problem:** Requires all peer addresses configured upfront

**Mitigation:**

- Make peer addresses optional in config
- Fall back to on-demand channel creation if missing
- Validate configuration before attempting channel creation

### Risk 4: Insufficient Funds

**Problem:** Connector wallet lacks ETH or tokens for deposits

**Mitigation:**

- Check balances before channel creation
- Log clear error messages
- Provide funding instructions in logs
- Continue startup without channels

## Alternative Approaches

### Alternative 1: Lazy Channel Creation (Current)

**Pros:**

- Simple implementation (already exists)
- No startup delay
- Only creates channels if actually needed

**Cons:**

- ❌ Channels don't exist when peer connects
- ❌ First settlement has extra latency
- ❌ Race conditions possible

### Alternative 2: Separate Channel Setup Script

**Approach:**
Create channels via separate script before starting connectors:

```bash
# Run before deployment
./scripts/setup-payment-channels.sh

# Then start connectors
docker compose up -d
```

**Pros:**

- Doesn't complicate connector startup
- Can validate channel creation independently
- Easier to troubleshoot

**Cons:**

- Extra operational step
- Channels not automatically maintained
- Requires manual intervention

### Alternative 3: Channel Creation Service

**Approach:**
Separate service that monitors peer connections and creates channels:

```typescript
// New service: channel-provisioner
class ChannelProvisioner {
  async onPeerConnected(peerId: string): Promise<void> {
    await this.ensureChannelExists(peerId);
  }
}
```

**Pros:**

- Decoupled from ConnectorNode
- Can be restarted independently
- Specialized responsibility

**Cons:**

- More complex architecture
- Additional service to deploy
- Extra communication overhead

## Recommendation

**Implement Phase 1-6 (Integrated Approach)**

**Why:**

- Most aligned with user expectation ("channels created when peer relationship is created")
- Clean architecture with proper separation of concerns
- Better operational visibility
- Manageable implementation effort

**Timeline:**

- Week 1: Phases 1-2 (settlement stack integration)
- Week 2: Phases 3-4 (configuration + error handling)
- Week 3: Phases 5-6 (telemetry + testing)

## Success Criteria

1. ✅ Channels created immediately when peer BTP connection succeeds
2. ✅ Connector logs show "Payment channel ready" for each peer at startup
3. ✅ Health endpoint shows channel status
4. ✅ First settlement uses existing channel (no on-chain delay)
5. ✅ Channel creation failures don't prevent connector startup
6. ✅ Explorer UI shows channel events

## Next Steps

1. **Review this design** with team/stakeholders
2. **Create story/task** for implementation
3. **Set up test environment** with funded wallets
4. **Implement Phase 1** (settlement stack integration)
5. **Iterate** through remaining phases

---

**This design addresses the architectural gap and provides a path to the correct behavior: channels created when peers connect.**
