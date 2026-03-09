# UnifiedSettlementExecutor API Reference

## Overview

The `UnifiedSettlementExecutor` class orchestrates EVM settlement on Base L2. It listens for `SETTLEMENT_REQUIRED` events from SettlementMonitor and routes settlements to the appropriate method based on peer configuration and token type.

**Key Features:**

- Automatic EVM (Base L2) settlement
- Peer-based settlement configuration
- Token type detection and routing
- Integration with TigerBeetle accounting layer
- Event-driven architecture

**Module:** `@crosstown/connector/settlement/unified-settlement-executor`

## Settlement Routing Logic

```
┌─────────────────────────────────────────────────────────────┐
│           Settlement Required Event                          │
│  { peerId, balance, tokenId }                               │
└──────────────────┬──────────────────────────────────────────┘
                   │
                   ▼
        ┌──────────────────────┐
        │  Get Peer Config     │
        └──────────┬───────────┘
                   │
                   ▼
        ┌──────────────────────┐
        │  EVM Settlement      │
        │  (PaymentChannel SDK)│
        └──────────────────────┘
```

## Types

### PeerConfig

```typescript
interface PeerConfig {
  /** Peer identifier */
  peerId: string;

  /** ILP address of peer */
  ilpAddress: string;

  /** Supported settlement tokens (ordered by preference) */
  settlementTokens: string[]; // e.g., ['USDC', 'DAI']

  /** EVM address for settlement */
  evmAddress: string;

  /** Settlement threshold (in base units) */
  settlementThreshold: bigint;

  /** Settlement interval (milliseconds) */
  settlementInterval: number;
}
```

### UnifiedSettlementExecutorConfig

```typescript
interface UnifiedSettlementExecutorConfig {
  /** Map of peer IDs to peer configurations */
  peers: Map<string, PeerConfig>;
}
```

### SettlementRequiredEvent

```typescript
interface SettlementRequiredEvent {
  /** Peer identifier */
  peerId: string;

  /** Balance to settle (string for bigint) */
  balance: string;

  /** Token identifier (ERC20 contract address) */
  tokenId: string;
}
```

## Constructor

### `new UnifiedSettlementExecutor(config, evmChannelSDK, settlementMonitor, accountManager, logger)`

Creates a new UnifiedSettlementExecutor instance.

**Parameters:**

- `config` **UnifiedSettlementExecutorConfig** - Unified settlement configuration with peer preferences
- `evmChannelSDK` **PaymentChannelSDK** - PaymentChannelSDK for EVM settlements (Epic 8)
- `settlementMonitor` **SettlementMonitor** - Settlement monitor emitting SETTLEMENT_REQUIRED events
- `accountManager` **AccountManager** - TigerBeetle account manager for balance updates
- `logger` **Logger** - Pino logger instance

**Example:**

```typescript
import { UnifiedSettlementExecutor } from '@crosstown/connector';

const config: UnifiedSettlementExecutorConfig = {
  peers: new Map([
    [
      'peer-alice',
      {
        peerId: 'peer-alice',
        ilpAddress: 'g.alice.connector',
        settlementTokens: ['USDC'],
        evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0',
        settlementThreshold: 1000000000n,
        settlementInterval: 3600000,
      },
    ],
  ]),
};

const executor = new UnifiedSettlementExecutor(
  config,
  evmChannelSDK,
  settlementMonitor,
  accountManager,
  logger
);
```

## Methods

### `start(): void`

Starts the settlement executor. Registers listener for `SETTLEMENT_REQUIRED` events from SettlementMonitor. Settlement routing begins after this method is called.

**Returns:** `void`

**Example:**

```typescript
executor.start();
logger.info('UnifiedSettlementExecutor started');
```

---

### `stop(): void`

Stops the settlement executor. Unregisters event listener and stops settlement processing. Ensures proper cleanup of event handlers.

**Returns:** `void`

**Example:**

```typescript
executor.stop();
logger.info('UnifiedSettlementExecutor stopped');
```

## Settlement Routing Rules

The executor applies the following routing logic:

### EVM Settlement

Triggered when:

- Settlement threshold reached for a peer
- Peer has `evmAddress` configured

**Actions:**

1. Open new EVM payment channel with peer
2. Deposit settlement amount to channel
3. Update TigerBeetle accounts

**Implementation:**

```typescript
// Routes to PaymentChannelSDK (Epic 8)
await evmChannelSDK.openChannel(peerAddress, tokenAddress, settlementTimeout, depositAmount);
```

### Error Cases

The executor throws an error when:

- **No peer configuration found:** Peer ID not in config map
- **Missing address:** Peer missing `evmAddress`

## Usage Examples

### Example 1: EVM Settlement Configuration

```typescript
import { UnifiedSettlementExecutor, PeerConfig } from '@crosstown/connector';

// Configure peers for EVM (Base L2) settlement
const config: UnifiedSettlementExecutorConfig = {
  peers: new Map([
    // Peer 1: USDC and DAI settlement
    [
      'peer-evm',
      {
        peerId: 'peer-evm',
        ilpAddress: 'g.peer1.connector',
        settlementTokens: ['USDC', 'DAI'],
        evmAddress: '0x123...',
        settlementThreshold: 1000000000n,
        settlementInterval: 3600000,
      },
    ],

    // Peer 2: USDC settlement
    [
      'peer-usdc',
      {
        peerId: 'peer-usdc',
        ilpAddress: 'g.peer2.connector',
        settlementTokens: ['USDC'],
        evmAddress: '0x456...',
        settlementThreshold: 1000000000n,
        settlementInterval: 3600000,
      },
    ],
  ]),
};

const executor = new UnifiedSettlementExecutor(
  config,
  evmChannelSDK,
  settlementMonitor,
  accountManager,
  logger
);

executor.start();
```

### Example 2: Settlement Event Flow

```typescript
// SettlementMonitor emits SETTLEMENT_REQUIRED event
settlementMonitor.emit('SETTLEMENT_REQUIRED', {
  peerId: 'peer-alice',
  balance: '5000000000', // 5,000 USDC in base units
  tokenId: '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48',
});

// UnifiedSettlementExecutor receives event and routes to EVM settlement
// (automatically handled when executor.start() has been called)

// Event handler internally:
// 1. Gets peer config
// 2. Opens EVM payment channel
// 3. Deposits settlement amount
// 4. Updates TigerBeetle accounts
```

### Example 3: Error Handling

```typescript
import { UnifiedSettlementExecutor } from '@crosstown/connector';

const executor = new UnifiedSettlementExecutor(
  config,
  evmChannelSDK,
  settlementMonitor,
  accountManager,
  logger
);

executor.start();

try {
  // Settlement event triggers routing logic
  settlementMonitor.emit('SETTLEMENT_REQUIRED', {
    peerId: 'unknown-peer',
    balance: '1000000',
    tokenId: '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48',
  });
} catch (error) {
  // Error: Peer configuration not found for peerId: unknown-peer
  logger.error({ error }, 'Settlement failed');
}
```

## Integration with SettlementMonitor

The UnifiedSettlementExecutor listens to the SettlementMonitor's `SETTLEMENT_REQUIRED` event:

```typescript
import { SettlementMonitor } from '@crosstown/connector';

// SettlementMonitor emits events when settlement thresholds reached
const settlementMonitor = new SettlementMonitor(config, accountManager, logger);

settlementMonitor.on('SETTLEMENT_REQUIRED', (event) => {
  // UnifiedSettlementExecutor handles this event automatically
  logger.info({ event }, 'Settlement required');
});

// Start both components
settlementMonitor.start();
executor.start();
```

## Integration with TigerBeetle

After successful settlement, the executor updates TigerBeetle accounts:

```typescript
// After EVM settlement completes
await accountManager.recordSettlement(peerId, tokenId, BigInt(balance));

// TigerBeetle accounts updated to reflect settled amount
```

## Cleanup

```typescript
// Stop executor before application shutdown
executor.stop();
settlementMonitor.stop();
```

## See Also

- [Payment Channel SDK Documentation](../guides/payment-channels.md) (Epic 8)
