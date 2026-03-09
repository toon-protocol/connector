# Multi-Hop ILP Network Architecture

## Network Topology

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                          5-Peer Linear Chain                                  │
│                                                                                │
│  ┌─────────┐      ┌─────────┐      ┌─────────┐      ┌─────────┐      ┌─────────┐  │
│  │  Peer1  │─────▶│  Peer2  │─────▶│  Peer3  │─────▶│  Peer4  │─────▶│  Peer5  │  │
│  │ :3000   │      │ :3001   │      │ :3002   │      │ :3003   │      │ :3004   │  │
│  └─────────┘      └─────────┘      └─────────┘      └─────────┘      └─────────┘  │
│   g.peer1         g.peer2          g.peer3          g.peer4          g.peer5      │
│   (Entry)         (Transit 1)      (Middle)         (Transit 3)      (Exit)       │
└──────────────────────────────────────────────────────────────────────────────┘
```

## Packet Flow

### PREPARE Packet (Forward Path)

```
┌─────────┐
│ Client  │  1. Create PREPARE packet
└────┬────┘      destination: g.peer5.dest
     │           amount: 1000000
     │           expiresAt: +30s
     ▼
┌─────────┐
│ Peer1   │  2. Route lookup: g.peer5 → nextHop: peer2
└────┬────┘     Decrement expiry: -1s
     │          Optional: Calculate fee
     │          Record settlement: incoming → peer2
     ▼
┌─────────┐
│ Peer2   │  3. Route lookup: g.peer5 → nextHop: peer3
└────┬────┘     Decrement expiry: -1s
     │          Forward to peer3
     ▼
┌─────────┐
│ Peer3   │  4. Route lookup: g.peer5 → nextHop: peer4
└────┬────┘     Decrement expiry: -1s
     │          Forward to peer4
     ▼
┌─────────┐
│ Peer4   │  5. Route lookup: g.peer5 → nextHop: peer5
└────┬────┘     Decrement expiry: -1s
     │          Forward to peer5
     ▼
┌─────────┐
│ Peer5   │  6. Route lookup: g.peer5 → nextHop: peer5 (local)
└────┬────┘     Validate packet
     │          Generate FULFILL
     │
     ▼
  FULFILL
```

### FULFILL Packet (Return Path)

```
┌─────────┐
│ Peer5   │  1. Generate fulfillment (preimage)
└────┬────┘     Return FULFILL to peer4
     │
     ▼
┌─────────┐
│ Peer4   │  2. Verify fulfillment
└────┬────┘     Update settlement
     │          Forward FULFILL to peer3
     ▼
┌─────────┐
│ Peer3   │  3. Verify fulfillment
└────┬────┘     Update settlement
     │          Forward FULFILL to peer2
     ▼
┌─────────┐
│ Peer2   │  4. Verify fulfillment
└────┬────┘     Update settlement
     │          Forward FULFILL to peer1
     ▼
┌─────────┐
│ Peer1   │  5. Verify fulfillment
└────┬────┘     Update settlement
     │          Return FULFILL to client
     ▼
┌─────────┐
│ Client  │  6. Receive FULFILL
└─────────┘     Payment complete!
```

## BTP Connections

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        Bilateral Transfer Protocol                           │
│                                                                               │
│  Peer1:3000  ◀────BTP────  Peer2 (client)                                   │
│      │                        │                                               │
│      │                        │                                               │
│      └────────────────────────┘                                               │
│                                                                               │
│  Peer2:3001  ◀────BTP────  Peer3 (client)                                   │
│      │                        │                                               │
│      │                        │                                               │
│      └────────────────────────┘                                               │
│                                                                               │
│  Peer3:3002  ◀────BTP────  Peer4 (client)                                   │
│      │                        │                                               │
│      │                        │                                               │
│      └────────────────────────┘                                               │
│                                                                               │
│  Peer4:3003  ◀────BTP────  Peer5 (client)                                   │
│      │                        │                                               │
│      │                        │                                               │
│      └────────────────────────┘                                               │
│                                                                               │
│  Each connection:                                                             │
│  - WebSocket transport                                                        │
│  - Shared secret authentication                                              │
│  - Request/response correlation                                              │
│  - Automatic retry with backoff                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Routing Tables

### Longest-Prefix Matching

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         Peer3 Routing Table                                  │
│                                                                               │
│  Prefix          NextHop      Priority    Description                        │
│  ──────────────  ───────────  ────────    ───────────────────────────────   │
│  g.peer1         peer2        0           Route upstream via peer2           │
│  g.peer2         peer2        0           Route upstream to peer2            │
│  g.peer3         peer3        0           Local delivery                     │
│  g.peer4         peer4        0           Route downstream to peer4          │
│  g.peer5         peer4        0           Route downstream via peer4         │
│                                                                               │
│  Lookup algorithm:                                                            │
│  1. Find all matching prefixes for destination                               │
│  2. Select longest matching prefix                                           │
│  3. If tie, use highest priority (lowest number)                             │
│  4. Return nextHop peer ID                                                    │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Treasury Funding

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           Treasury Wallet                                    │
│                    (Loaded from TREASURY_EVM_PRIVATE_KEY)                   │
│                                                                               │
│  ┌─────────────────────────────────────────────────────────────┐            │
│  │  Balance:                                                    │            │
│  │  - ETH: 10.0 (for gas fees)                                 │            │
│  │  - M2M Token: 100,000 (for settlement)                      │            │
│  └─────────────────────────────────────────────────────────────┘            │
│                                                                               │
│         │              │              │              │              │         │
│         ▼              ▼              ▼              ▼              ▼         │
│    ┌────────┐    ┌────────┐    ┌────────┐    ┌────────┐    ┌────────┐     │
│    │ Peer1  │    │ Peer2  │    │ Peer3  │    │ Peer4  │    │ Peer5  │     │
│    │ Wallet │    │ Wallet │    │ Wallet │    │ Wallet │    │ Wallet │     │
│    └────────┘    └────────┘    └────────┘    └────────┘    └────────┘     │
│     0.1 ETH      0.1 ETH      0.1 ETH      0.1 ETH      0.1 ETH            │
│    1000 M2M     1000 M2M     1000 M2M     1000 M2M     1000 M2M            │
│                                                                               │
│  Funding process:                                                             │
│  1. Generate or load peer addresses                                          │
│  2. For each peer:                                                            │
│     a. Send ETH for gas (treasuryWallet.sendTransaction)                    │
│     b. Send M2M tokens for settlement (tokenContract.transfer)              │
│  3. Wait for transaction confirmations                                       │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Payment Channels

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        Payment Channel Network                               │
│                                                                               │
│  Peer1 ◀────────▶ Peer2 ◀────────▶ Peer3 ◀────────▶ Peer4 ◀────────▶ Peer5│
│    │                 │                 │                 │                 │  │
│    │  Channel 1-2    │   Channel 2-3   │   Channel 3-4   │   Channel 4-5   │  │
│    │  ID: 0xabc...   │   ID: 0xdef...  │   ID: 0x123...  │   ID: 0x456...  │  │
│    │  Deposit: 10k   │   Deposit: 10k  │   Deposit: 10k  │   Deposit: 10k  │  │
│    │  Timeout: 24h   │   Timeout: 24h  │   Timeout: 24h  │   Timeout: 24h  │  │
│    └─────────────────┴─────────────────┴─────────────────┴─────────────────┘  │
│                                                                               │
│  Each channel:                                                                │
│  - ERC20 TokenNetwork contract                                               │
│  - Initial deposit = settlementThreshold × 10                                │
│  - Off-chain balance proofs (claims)                                         │
│  - Automatic refunding when balance < threshold × 0.5                        │
│  - Cooperative or unilateral closure                                         │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Claim-Based Settlement

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         Off-Chain Balance Proofs                             │
│                                                                               │
│  Balance Proof Structure:                                                    │
│  ┌──────────────────────────────────────────────────────────────┐           │
│  │  {                                                             │           │
│  │    channelId: "0xabc...",        // Channel identifier        │           │
│  │    nonce: 42,                    // Monotonically increasing  │           │
│  │    transferredAmount: 5000n,     // Cumulative sent          │           │
│  │    lockedAmount: 0n,             // Amount in pending HTLCs  │           │
│  │    locksRoot: "0x000..."         // Merkle root of locks     │           │
│  │  }                                                             │           │
│  └──────────────────────────────────────────────────────────────┘           │
│                                                                               │
│  Signing Process:                                                             │
│  1. Create claim message from balance proof                                  │
│  2. Sign with KeyManager (supports HSM/KMS):                                 │
│     const message = createClaimMessage(channelId, amount);                   │
│     const signature = await keyManager.sign(message, keyId);                 │
│  3. Exchange signed claims off-chain between peers                           │
│  4. Only submit to blockchain on dispute or channel closure                  │
│                                                                               │
│  Verification:                                                                │
│  1. Check signature validity (ed25519 or secp256k1)                         │
│  2. Verify nonce > previous nonce (monotonic increase)                       │
│  3. Validate transferredAmount ≤ channel capacity                            │
│  4. Check claim hasn't expired (if timestamp present)                        │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Deployment Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                          Docker Compose Stack                                │
│                                                                               │
│  ┌─────────────────────────────────────────────────────────────────────┐   │
│  │  Docker Network: ilp-network (bridge)                               │   │
│  │                                                                       │   │
│  │  ┌────────┐  ┌────────┐  ┌────────┐  ┌────────┐  ┌────────┐       │   │
│  │  │ peer1  │  │ peer2  │  │ peer3  │  │ peer4  │  │ peer5  │       │   │
│  │  │ :3000  │  │ :3001  │  │ :3002  │  │ :3003  │  │ :3004  │       │   │
│  │  │ :8080  │  │ :8080  │  │ :8080  │  │ :8080  │  │ :8080  │       │   │
│  │  └────────┘  └────────┘  └────────┘  └────────┘  └────────┘       │   │
│  │                                                                       │   │
│  │  ┌────────────────────────────────────────────────────────┐         │   │
│  │  │  anvil (local blockchain)                               │         │   │
│  │  │  :8545 - JSON-RPC endpoint                             │         │   │
│  │  └────────────────────────────────────────────────────────┘         │   │
│  └─────────────────────────────────────────────────────────────────────┘   │
│                                                                               │
│  Port Mappings (Host → Container):                                           │
│  - 3000 → peer1:3000 (BTP)                                                   │
│  - 3001 → peer2:3001 (BTP)                                                   │
│  - 3002 → peer3:3002 (BTP)                                                   │
│  - 3003 → peer4:3003 (BTP)                                                   │
│  - 3004 → peer5:3004 (BTP)                                                   │
│  - 9080 → peer1:8080 (Health)                                                │
│  - 9081 → peer2:8080 (Health)                                                │
│  - 9082 → peer3:8080 (Health)                                                │
│  - 9083 → peer4:8080 (Health)                                                │
│  - 9084 → peer5:8080 (Health)                                                │
│  - 8545 → anvil:8545 (RPC)                                                   │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Deployment Flow

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                    deploy-5-peer-multihop.sh                                 │
│                                                                               │
│  [1/6] Check Prerequisites                                                   │
│         │                                                                     │
│         ├─ Docker running?                                                   │
│         ├─ Docker Compose installed?                                         │
│         ├─ Connector image built?                                            │
│         └─ .env file configured?                                             │
│         │                                                                     │
│  [2/6] Start Network                                                         │
│         │                                                                     │
│         ├─ docker-compose up -d                                              │
│         ├─ Wait for containers                                               │
│         └─ Health check each peer                                            │
│         │                                                                     │
│  [3/6] Fund Peers                                                            │
│         │                                                                     │
│         ├─ Load treasury wallet                                              │
│         ├─ Generate peer addresses (if needed)                               │
│         ├─ Send ETH to each peer                                             │
│         └─ Send M2M tokens to each peer                                      │
│         │                                                                     │
│  [4/6] Display Topology                                                      │
│         │                                                                     │
│         └─ Show network diagram                                              │
│         │                                                                     │
│  [5/6] Send Test Packet                                                      │
│         │                                                                     │
│         ├─ Connect to Peer1 via BTP                                          │
│         ├─ Send PREPARE to g.peer5.dest                                      │
│         └─ Receive FULFILL response                                          │
│         │                                                                     │
│  [6/6] Verify Multi-Hop                                                      │
│         │                                                                     │
│         ├─ Check Peer1-4 logs for PREPARE forwarding                         │
│         ├─ Check Peer5 logs for local delivery                               │
│         ├─ Check all logs for FULFILL propagation                            │
│         └─ Report success/failure                                            │
│         │                                                                     │
│         ▼                                                                     │
│    ✓ SUCCESS!                                                                │
└─────────────────────────────────────────────────────────────────────────────┘
```
