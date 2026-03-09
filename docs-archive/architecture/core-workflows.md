# Core Workflows

## Packet Forwarding Workflow (Multi-Hop)

The following sequence diagram illustrates the core ILP packet forwarding flow through multiple connector hops with telemetry emission:

```mermaid
sequenceDiagram
    participant Sender as Test Packet Sender
    participant ConnA as Connector A
    participant ConnB as Connector B
    participant ConnC as Connector C

    Note over Sender,ConnC: Scenario: Send packet from A to C via B

    Sender->>ConnA: Send ILP Prepare (destination: g.connectorC.dest)
    activate ConnA
    ConnA->>ConnA: BTPServer receives packet
    ConnA->>ConnA: PacketHandler.validatePacket()
    ConnA->>ConnA: RoutingTable.lookup("g.connectorC.dest")
    ConnA->>ConnA: Result: nextHop = "connectorB"
    ConnA->>ConnA: Log: PACKET_RECEIVED
    ConnA->>ConnA: Log: ROUTE_LOOKUP (peer=connectorB)
    ConnA->>ConnB: BTPClient.sendPacket() via WebSocket
    ConnA->>ConnA: Log: PACKET_SENT (nextHop=connectorB)
    deactivate ConnA

    activate ConnB
    ConnB->>ConnB: BTPServer receives packet
    ConnB->>ConnB: PacketHandler.validatePacket()
    ConnB->>ConnB: RoutingTable.lookup("g.connectorC.dest")
    ConnB->>ConnB: Result: nextHop = "connectorC"
    ConnB->>ConnB: Log: PACKET_RECEIVED
    ConnB->>ConnB: Log: ROUTE_LOOKUP (peer=connectorC)
    ConnB->>ConnC: BTPClient.sendPacket() via WebSocket
    ConnB->>ConnB: Log: PACKET_SENT (nextHop=connectorC)
    deactivate ConnB

    activate ConnC
    ConnC->>ConnC: BTPServer receives packet
    ConnC->>ConnC: PacketHandler.validatePacket()
    ConnC->>ConnC: Packet delivered (destination reached)
    ConnC->>ConnC: Log: PACKET_RECEIVED
    ConnC->>ConnB: ILP Fulfill (propagate back)
    deactivate ConnC

    activate ConnB
    ConnB->>ConnA: ILP Fulfill (propagate back)
    deactivate ConnB

    activate ConnA
    ConnA->>Sender: ILP Fulfill (final response)
    deactivate ConnA

    Note over ConnA,ConnC: Telemetry events logged to stdout
```

## Per-Hop BLS Notification Pipeline

Every connector in the path can notify its local Business Logic Server (BLS) when a packet transits through. Intermediate hops fire-and-forget the notification (non-blocking), while the final hop awaits a fulfill/reject decision from its BLS.

```mermaid
sequenceDiagram
    participant Sender as Sender
    participant ConnA as Connector A
    participant BLS_A as BLS A
    participant ConnB as Connector B
    participant BLS_B as BLS B
    participant ConnC as Connector C (Final Hop)
    participant BLS_C as BLS C

    Note over Sender,BLS_C: Packet destination: g.connectorC.dest

    Sender->>ConnA: ILP Prepare
    activate ConnA
    ConnA->>ConnA: RoutingTable.lookup() → nextHop = connectorB

    ConnA-)BLS_A: POST /handle-packet (fire-and-forget)
    Note right of BLS_A: Non-blocking. BLS does<br/>computation, logging,<br/>analytics — no response needed.

    ConnA->>ConnB: Forward via BTP (critical path)
    deactivate ConnA

    activate ConnB
    ConnB->>ConnB: RoutingTable.lookup() → nextHop = connectorC

    ConnB-)BLS_B: POST /handle-packet (fire-and-forget)
    Note right of BLS_B: Non-blocking. Same payload<br/>format as final-hop delivery.

    ConnB->>ConnC: Forward via BTP (critical path)
    deactivate ConnB

    activate ConnC
    ConnC->>ConnC: RoutingTable.lookup() → nextHop = local

    ConnC->>BLS_C: POST /handle-packet (await response)
    activate BLS_C
    BLS_C-->>ConnC: { accept: true }
    deactivate BLS_C
    Note right of BLS_C: Blocking. BLS decides<br/>accept/reject. Connector<br/>computes fulfillment.

    ConnC-->>ConnB: ILP Fulfill
    deactivate ConnC

    activate ConnB
    ConnB-->>ConnA: ILP Fulfill
    deactivate ConnB

    activate ConnA
    ConnA-->>Sender: ILP Fulfill
    deactivate ConnA
```

### Key Behaviors

- **Intermediate hops**: `localDeliveryClient.deliver()` is called with `.catch(noop)` — no `await`, no impact on the critical forwarding path
- **Final hop**: `localDeliveryClient.deliver()` is awaited — the BLS response determines whether to return ILP FULFILL or ILP REJECT
- **Same payload**: Every BLS receives the same `PaymentRequest` format (`paymentId`, `destination`, `amount`, `expiresAt`, `data`) regardless of hop position
- **No packet modification**: The ILP packet is forwarded unchanged to the next hop — the BLS notification is a pure side-effect
- **Failure isolation**: If a fire-and-forget POST fails at an intermediate hop, the packet forwarding is unaffected

## Telemetry and Observability Workflow

**Note:** Dashboard visualization deferred - see DASHBOARD-DEFERRED.md in root

```mermaid
sequenceDiagram
    participant Conn as Connector Nodes (A, B, C)
    participant Logger as Structured Logger
    participant Stdout as Standard Output

    Note over Conn,Stdout: Runtime Telemetry Emission

    Conn->>Logger: Emit: NODE_STATUS (routes, peers)
    Logger->>Stdout: JSON structured log entry

    Conn->>Logger: Emit: PACKET_RECEIVED (packetId, details)
    Logger->>Stdout: JSON structured log entry

    Conn->>Logger: Emit: PACKET_SENT (packetId, nextHop)
    Logger->>Stdout: JSON structured log entry

    Conn->>Logger: Emit: ROUTE_LOOKUP (destination, selectedPeer)
    Logger->>Stdout: JSON structured log entry

    Note over Stdout: Logs consumable by external monitoring tools
```

## Connector Startup and BTP Connection Establishment

```mermaid
sequenceDiagram
    participant Docker as Docker Compose
    participant ConnA as Connector A
    participant ConnB as Connector B

    Note over Docker,ConnB: Startup Sequence

    Docker->>ConnA: Start connector-a container
    activate ConnA
    ConnA->>ConnA: Load config.yaml (routes, peers)
    ConnA->>ConnA: Initialize RoutingTable from config
    ConnA->>ConnA: Start BTPServer (port 3000)
    ConnA->>ConnA: Health check: STARTING
    deactivate ConnA

    Docker->>ConnB: Start connector-b container
    activate ConnB
    ConnB->>ConnB: Load config.yaml
    ConnB->>ConnB: Initialize RoutingTable
    ConnB->>ConnB: Start BTPServer (port 3000)
    deactivate ConnB

    Note over ConnA,ConnB: BTP Peer Connection Phase

    activate ConnA
    ConnA->>ConnB: BTPClient connects (ws://connector-b:3000)
    ConnB->>ConnA: BTP AUTH response (handshake)
    ConnA->>ConnA: Mark peer "connectorB" as CONNECTED
    ConnA->>ConnA: Health check: READY
    deactivate ConnA

    activate ConnB
    ConnB->>ConnA: BTPClient connects (ws://connector-a:3000)
    ConnA->>ConnB: BTP AUTH response
    ConnB->>ConnB: Mark peer "connectorA" as CONNECTED
    ConnB->>ConnB: Health check: READY
    deactivate ConnB

    Note over ConnA,ConnB: Telemetry Emission

    ConnA->>ConnA: Emit: NODE_STATUS (routes, peers)
    ConnB->>ConnB: Emit: NODE_STATUS (routes, peers)

    Note over Docker: All containers healthy - system operational
```

## EVM Settlement Routing Workflow

```mermaid
flowchart TD
    Start[Settlement Required Event] --> GetPeer[Get Peer Config]
    GetPeer --> CheckEVM{Peer has evmAddress?}

    CheckEVM -->|Yes| OpenChannel[Open/Use EVM Payment Channel]
    CheckEVM -->|No| Error[Error: No compatible method]

    OpenChannel --> Deposit[Deposit ERC20 Tokens]
    Deposit --> UpdateAccounts[Update TigerBeetle Accounts]
    UpdateAccounts --> Done[Settlement Complete]

    style Start fill:#2563eb,color:#fff
    style OpenChannel fill:#059669,color:#fff
    style Error fill:#dc2626,color:#fff
    style Done fill:#16a34a,color:#fff
```

## Claim Exchange Workflow (Epic 17)

Off-chain balance proof exchange between peers via BTP `payment-channel-claim` sub-protocol. This is the core settlement mechanism — signed claims accumulate off-chain and are redeemed on-chain only when economically optimal.

```mermaid
sequenceDiagram
    participant SM as SettlementMonitor
    participant USE as UnifiedSettlementExecutor
    participant AM as AccountManager
    participant SDK as PaymentChannelSDK
    participant CS as ClaimSender
    participant BTP as BTP WebSocket
    participant CR as ClaimReceiver (Peer)
    participant CRS as ClaimRedemptionService (Peer)

    Note over SM,CRS: Settlement threshold reached for peer-token pair

    SM->>SM: Poll AccountManager balances (30s interval)
    SM->>AM: getBalance(peerId, tokenId)
    AM-->>SM: { netBalance: 5000000n }
    SM->>SM: netBalance > threshold → state: SETTLEMENT_PENDING

    SM--)USE: emit SETTLEMENT_REQUIRED { peerId, balance, tokenId }
    activate USE

    USE->>SDK: signBalanceProof(channelId, tokenNetworkAddr, nonce, amount, locked, locksRoot)
    SDK-->>USE: signature (EIP-712 typed data)

    USE->>CS: sendEVMClaim(peerId, btpClient, channelId, nonce, amount, ...)
    activate CS

    CS->>CS: Build EVMClaimMessage { version, blockchain, channelId, nonce, signature, ... }
    CS->>CS: Persist claim to SQLite (dispute resolution)
    CS->>BTP: protocolName: 'payment-channel-claim', data: JSON claim
    BTP->>CR: Receive BTP message
    deactivate CS

    activate CR
    CR->>CR: validateClaimMessage() — structure validation
    CR->>CR: Check nonce > lastNonce (monotonicity)
    CR->>SDK: verifyBalanceProof() — EIP-712 signature verification
    SDK-->>CR: signerAddress (recovered)
    CR->>CR: Persist verified claim to SQLite
    CR->>CR: Emit CLAIM_VERIFIED telemetry
    deactivate CR

    Note over CRS: ClaimRedemptionService polls every 30s

    CRS->>CRS: Check if profitable to redeem on-chain
    alt Profitable to redeem
        CRS->>SDK: closeChannel() or cooperativeSettle()
        SDK-->>CRS: On-chain settlement tx
    else Wait for more claims
        CRS->>CRS: Continue accumulating
    end

    USE->>AM: recordSettlement(peerId, tokenId, amount)
    deactivate USE
    SM->>SM: state: IDLE
```

### Key Behaviors

- **Off-chain by default**: Claims are signed balance proofs exchanged over WebSocket — no on-chain transaction per claim
- **Nonce monotonicity**: Each claim must have a higher nonce than the previous, preventing replay attacks
- **Cumulative amounts**: `transferredAmount` is cumulative (not incremental), so only the latest claim matters for on-chain settlement
- **Retry with backoff**: ClaimSender retries failed sends with exponential backoff (1s, 2s, 4s — 3 attempts)
- **Dispute resolution**: Both sender and receiver persist claims to SQLite, enabling on-chain dispute if needed
- **Deferred redemption**: ClaimRedemptionService batches claims and redeems on-chain only when gas-efficient

## Dynamic On-Chain Verification Workflow (Epic 31)

Self-describing claims enable unknown peers to send claims with embedded chain/contract coordinates. The receiver verifies the channel on-chain dynamically, eliminating the need for SPSP handshake (kind:23194/23195) and Admin API channel pre-registration.

```mermaid
sequenceDiagram
    participant Peer as Unknown Peer
    participant BTP as BTP WebSocket
    participant CR as ClaimReceiver
    participant CM as ChannelManager
    participant SDK as PaymentChannelSDK
    participant RPC as EVM RPC (Base L2)

    Note over Peer,RPC: Peer sends self-describing claim (no prior handshake)

    Peer->>BTP: Connect via BTP WebSocket
    BTP->>CR: payment-channel-claim message

    activate CR
    CR->>CR: validateClaimMessage() — structure validation
    CR->>CR: Extract self-describing fields: chainId, tokenNetworkAddress, tokenAddress

    CR->>CM: getChannelMetadata(channelId)
    CM-->>CR: null (unknown channel)

    Note over CR,RPC: First-time channel — dynamic on-chain verification

    CR->>SDK: getChannelState(channelId, tokenNetworkAddress)
    activate SDK
    SDK->>RPC: channels(channelId) — read on-chain state
    RPC-->>SDK: { state: 'opened', participant1, participant2, settlementTimeout }
    deactivate SDK

    CR->>CR: Verify sender is a channel participant
    CR->>CR: Verify channel state is 'opened'

    CR->>SDK: verifyBalanceProof(channelId, tokenNetworkAddress, nonce, amount, locked, locksRoot, signature)
    Note right of SDK: EIP-712 domain constructed from<br/>self-describing claim fields:<br/>chainId + tokenNetworkAddress

    SDK-->>CR: signerAddress (recovered from EIP-712)
    CR->>CR: Verify signerAddress matches channel participant

    Note over CR,CM: Auto-register channel and peer

    CR->>CM: registerChannel({ channelId, peerId, tokenId, tokenAddress, chain, status: 'open' })
    CM->>CM: Cache in channelMetadata + peerChannelIndex

    CR->>CR: Persist verified claim to SQLite
    CR->>CR: Emit CLAIM_VERIFIED telemetry
    deactivate CR

    Note over Peer,RPC: Subsequent claims — fast path (cached)

    Peer->>BTP: Second payment-channel-claim
    activate CR
    CR->>CM: getChannelMetadata(channelId)
    CM-->>CR: ChannelMetadata (cached)
    CR->>CR: Skip RPC — verify EIP-712 signature only
    CR->>CR: Check nonce > lastNonce
    CR-->>CR: Verified (fast path)
    deactivate CR
```

### Key Behaviors

- **Self-describing claims**: EVMClaimMessage includes optional `chainId`, `tokenNetworkAddress`, `tokenAddress` fields — the claim carries everything needed for verification
- **First-time RPC verification**: Unknown channels trigger a one-time on-chain read to confirm the channel exists and the sender is a participant
- **EIP-712 domain from claim**: The typed data domain is constructed from the claim's own `chainId` and `tokenNetworkAddress`, not from connector config
- **Auto-registration**: Verified channels are automatically registered in ChannelManager's cache and the peer is associated
- **Cached fast path**: Subsequent claims for the same channel skip RPC and verify the EIP-712 signature directly
- **SPSP elimination**: No Nostr kind:23194/23195 exchange needed — the claim itself carries the contract coordinates
- **Admin API bypass**: No `POST /admin/peers` required to pre-register channels — dynamic verification replaces manual setup
