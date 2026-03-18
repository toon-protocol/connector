# @toon-protocol/shared

Shared ILP types and OER codec for Connector.

## Install

```bash
npm install @toon-protocol/shared
```

## Usage

### ILP Packets

```typescript
import {
  ILPPreparePacket,
  ILPFulfillPacket,
  PacketType,
  serializePacket,
  deserializePacket,
} from '@toon-protocol/shared';

// Create an ILP Prepare packet
const prepare: ILPPreparePacket = {
  type: PacketType.Prepare,
  amount: BigInt(1000),
  expiresAt: new Date(Date.now() + 30000),
  executionCondition: Buffer.alloc(32),
  destination: 'g.example.receiver',
  data: Buffer.alloc(0),
};

// Serialize to binary (OER encoding per RFC-0030)
const encoded: Buffer = serializePacket(prepare);

// Deserialize back
const decoded = deserializePacket(encoded);
```

### Telemetry Events

All telemetry events use a discriminated union on the `type` field. Switch on `event.type` to narrow to a specific interface:

```typescript
import { TelemetryEvent, TelemetryEventType } from '@toon-protocol/shared';

function handleEvent(event: TelemetryEvent) {
  switch (event.type) {
    case 'PACKET_FORWARDED':
      console.log(`Packet forwarded to ${event.to}`);
      break;
    case 'PER_HOP_NOTIFICATION':
      console.log(`Transit notification: ${event.destination} via ${event.nextHop}`);
      break;
    case 'SETTLEMENT_COMPLETED':
      console.log(`Settlement ${event.success ? 'ok' : 'failed'}: ${event.peerId}`);
      break;
  }
}
```

## Exported Types

### ILP Packets

Types for Interledger Protocol v4 packets (RFC-0027):

- `ILPPreparePacket`, `ILPFulfillPacket`, `ILPRejectPacket`, `ILPPacket`
- `PacketType` enum, `ILPErrorCode` enum
- `isPreparePacket()`, `isFulfillPacket()`, `isRejectPacket()` type guards
- `isValidILPAddress()` validation (RFC-0015)

### OER Encoding (RFC-0030)

Binary serialization for ILP packets:

- `serializePacket()` / `deserializePacket()` — generic packet codec
- `serializePrepare()` / `deserializePrepare()` — Prepare-specific
- `serializeFulfill()` / `deserializeFulfill()` — Fulfill-specific
- `serializeReject()` / `deserializeReject()` — Reject-specific
- `encodeVarUInt()`, `decodeVarOctetString()`, etc. — OER primitives

### Routing

- `RoutingTableEntry` — prefix-to-nextHop mapping

### Telemetry Events

All events share a `type` discriminator and are combined in the `TelemetryEvent` union:

| Event Type                       | Interface                          | Description                                             |
| -------------------------------- | ---------------------------------- | ------------------------------------------------------- |
| `PACKET_RECEIVED`                | `PacketReceivedEvent`              | ILP packet received from peer                           |
| `PACKET_FORWARDED`               | `PacketForwardedEvent`             | ILP packet forwarded to next hop                        |
| `PACKET_FULFILLED`               | `PacketFulfilledEvent`             | ILP packet successfully fulfilled                       |
| `PACKET_REJECTED`                | `PacketRejectedEvent`              | ILP packet rejected with error code                     |
| `PER_HOP_NOTIFICATION`           | `PerHopNotificationEvent`          | Transit BLS notification dispatched at intermediate hop |
| `ACCOUNT_BALANCE`                | `AccountBalanceEvent`              | Account balance changed                                 |
| `SETTLEMENT_TRIGGERED`           | `SettlementTriggeredEvent`         | Settlement threshold exceeded                           |
| `SETTLEMENT_COMPLETED`           | `SettlementCompletedEvent`         | Settlement execution completed                          |
| `CLAIM_SENT`                     | `ClaimSentEvent`                   | Payment channel claim sent via BTP                      |
| `CLAIM_RECEIVED`                 | `ClaimReceivedEvent`               | Payment channel claim received via BTP                  |
| `CLAIM_REDEEMED`                 | `ClaimRedeemedEvent`               | Claim redeemed on-chain                                 |
| `PAYMENT_CHANNEL_OPENED`         | `PaymentChannelOpenedEvent`        | EVM payment channel opened                              |
| `PAYMENT_CHANNEL_BALANCE_UPDATE` | `PaymentChannelBalanceUpdateEvent` | Off-chain balance proof updated                         |
| `PAYMENT_CHANNEL_SETTLED`        | `PaymentChannelSettledEvent`       | EVM channel settled on-chain                            |
| `AGENT_BALANCE_CHANGED`          | `AgentBalanceChangedEvent`         | Agent wallet balance changed                            |
| `AGENT_WALLET_FUNDED`            | `AgentWalletFundedEvent`           | Agent wallet received funding                           |
| `AGENT_CHANNEL_OPENED`           | `AgentChannelOpenedEvent`          | Agent payment channel opened                            |
| `AGENT_CHANNEL_PAYMENT_SENT`     | `AgentChannelPaymentSentEvent`     | Agent sent payment through channel                      |
| `AGENT_CHANNEL_BALANCE_UPDATE`   | `AgentChannelBalanceUpdateEvent`   | Agent channel balance changed                           |
| `AGENT_CHANNEL_CLOSED`           | `AgentChannelClosedEvent`          | Agent payment channel closed                            |

### Payment Channel Types

- `ChannelState`, `ChannelStatus`, `BalanceProof`
- `ChannelOpenedEvent`, `ChannelClosedEvent`, `ChannelSettledEvent`

## What's New in 1.1.0

- Added `PerHopNotificationEvent` telemetry type — emitted when a connector dispatches a fire-and-forget BLS notification at an intermediate hop
- Added `PER_HOP_NOTIFICATION` to `TelemetryEventType` enum

## Monorepo

This package is part of the [connector](https://github.com/ALLiDoizCode/connector) monorepo.

## License

MIT
