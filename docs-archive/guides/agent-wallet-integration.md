# Agent Wallet Integration Guide

A comprehensive guide to integrating agent wallets into your AI agent applications, enabling micropayments and multi-chain cryptocurrency support.

## Table of Contents

1. [Quick Start (5 Minutes)](#quick-start-5-minutes)
2. [Wallet Creation](#wallet-creation)
3. [Wallet Funding](#wallet-funding)
4. [Balance Queries](#balance-queries)
5. [Payment Channel Usage](#payment-channel-usage)
6. [Integration Patterns](#integration-patterns)

---

## Quick Start (5 Minutes)

Get your first agent wallet up and running in just 5 minutes.

### Prerequisites

- Node.js 20.11.0+ installed
- TypeScript 5.3.3+ configured
- Access to blockchain RPC endpoints (Base L2)

### Step 1: Create Agent Wallet

```typescript
import { AgentWalletLifecycle } from '@crosstown/connector/wallet/agent-wallet-lifecycle';
import { pino } from 'pino';

const logger = pino({ level: 'info' });

async function createAgentWallet() {
  const lifecycle = new AgentWalletLifecycle();

  try {
    const wallet = await lifecycle.createAgentWallet('agent-001');
    logger.info('Agent wallet created', {
      agentId: wallet.agentId,
      evmAddress: wallet.evmAddress,
      status: wallet.status,
    });
    return wallet;
  } catch (error) {
    logger.error('Wallet creation failed', { error: error.message });
    throw error;
  }
}

createAgentWallet();
```

**Expected Output:**

```json
{
  "level": "info",
  "msg": "Agent wallet created",
  "agentId": "agent-001",
  "evmAddress": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb",
  "status": "active"
}
```

### Step 2: Check Balance

```typescript
import { AgentBalanceTracker } from '@crosstown/connector/wallet/agent-balance-tracker';

async function checkBalance(agentId: string) {
  const balanceTracker = new AgentBalanceTracker();

  try {
    // Get all balances for the agent
    const balances = await balanceTracker.getAllBalances(agentId);

    logger.info('Agent balances retrieved', {
      agentId,
      balances: balances.map((b) => ({
        chain: b.chain,
        token: b.token,
        balance: b.balance.toString(),
      })),
    });

    return balances;
  } catch (error) {
    logger.error('Balance check failed', { error: error.message });
    throw error;
  }
}

checkBalance('agent-001');
```

**Expected Output:**

```json
{
  "level": "info",
  "msg": "Agent balances retrieved",
  "agentId": "agent-001",
  "balances": [
    { "chain": "evm", "token": "ETH", "balance": "100000000000000000" },
    {
      "chain": "evm",
      "token": "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48",
      "balance": "1000000000"
    }
  ]
}
```

### Step 3: Open Payment Channel

```typescript
import { AgentChannelManager } from '@crosstown/connector/wallet/agent-channel-manager';

async function openPaymentChannel(agentId: string) {
  const channelManager = new AgentChannelManager();

  try {
    const channelId = await channelManager.openChannel(
      agentId,
      'peer-agent-002', // Peer agent ID
      'evm', // Chain
      'USDC', // Token
      BigInt(1000000000) // Amount: 1000 USDC (6 decimals)
    );

    logger.info('Payment channel opened', {
      agentId,
      channelId,
      peerId: 'peer-agent-002',
      chain: 'evm',
      token: 'USDC',
      amount: '1000000000',
    });

    return channelId;
  } catch (error) {
    logger.error('Channel opening failed', { error: error.message });
    throw error;
  }
}

openPaymentChannel('agent-001');
```

**Expected Output:**

```json
{
  "level": "info",
  "msg": "Payment channel opened",
  "agentId": "agent-001",
  "channelId": "channel-evm-001",
  "peerId": "peer-agent-002",
  "chain": "evm",
  "token": "USDC",
  "amount": "1000000000"
}
```

### Step 4: Send Payment

```typescript
async function sendPayment(agentId: string, channelId: string) {
  const channelManager = new AgentChannelManager();

  try {
    await channelManager.sendPayment(
      agentId,
      channelId,
      BigInt(50000000) // Amount: 50 USDC
    );

    logger.info('Payment sent', {
      agentId,
      channelId,
      amount: '50000000',
    });
  } catch (error) {
    logger.error('Payment failed', { error: error.message });
    throw error;
  }
}

sendPayment('agent-001', 'channel-evm-001');
```

**Expected Output:**

```json
{
  "level": "info",
  "msg": "Payment sent",
  "agentId": "agent-001",
  "channelId": "channel-evm-001",
  "amount": "50000000"
}
```

---

## Wallet Creation

Agent wallets are hierarchical deterministic (HD) wallets derived from a master seed using BIP-32/BIP-44 standards. Each agent receives a unique address for EVM (Base L2).

### How Wallet Derivation Works

The wallet system uses a master seed to deterministically generate unique addresses for each agent:

1. **Master Seed**: A 24-word BIP-39 mnemonic phrase stored securely (encrypted at rest with AES-256-GCM)
2. **Derivation Path**:
   - EVM addresses: `m/44'/60'/1'/0/{agentIndex}` (Ethereum standard)
3. **Agent Index**: Each agent is assigned a unique index (0 to 2^31 - 1)

### Creating an Agent Wallet

```typescript
import { AgentWalletLifecycle } from '@crosstown/connector/wallet/agent-wallet-lifecycle';

const lifecycle = new AgentWalletLifecycle();

// Create wallet for new agent
const wallet = await lifecycle.createAgentWallet('agent-001');

// Wallet structure
interface AgentWallet {
  agentId: string; // Unique agent identifier
  evmAddress: string; // EVM address (0x-prefixed, 42 chars)
  derivationIndex: number; // HD wallet index
  createdAt: Date;
  status: WalletStatus; // 'pending' | 'active' | 'suspended' | 'archived'
}
```

### Wallet Lifecycle States

Agent wallets transition through the following states:

1. **Pending**: Wallet created but not yet funded (initial state)
2. **Active**: Wallet funded and ready for transactions
3. **Suspended**: Temporarily disabled (can be reactivated)
4. **Archived**: Permanently archived after inactivity

```typescript
// Get existing wallet
const existingWallet = await lifecycle.getAgentWallet('agent-001');

if (!existingWallet) {
  // Wallet doesn't exist, create new one
  const newWallet = await lifecycle.createAgentWallet('agent-001');
}

// Suspend wallet temporarily
await lifecycle.suspendWallet('agent-001', 'Security review pending');

// Archive inactive wallet
const archive = await lifecycle.archiveWallet('agent-001');
```

### EVM Address Output

Each agent receives an EVM (Base L2) address:

```typescript
const wallet = await lifecycle.createAgentWallet('agent-001');

// EVM (Base L2) address
// Format: 0x-prefixed hex (42 characters)
// Example: 0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb
logger.info('EVM address', { address: wallet.evmAddress });
```

---

## Wallet Funding

New agent wallets are automatically funded upon creation to ensure they have sufficient balance for transactions.

### Automatic Funding on Creation

When you create an agent wallet, the system automatically funds it with:

1. **EVM (Base L2)**:
   - **Native ETH**: 0.1 ETH (for gas fees)
   - **USDC tokens**: 1000 USDC (for initial payments)

### Checking Funding Status

```typescript
import { AgentWalletLifecycle } from '@crosstown/connector/wallet/agent-wallet-lifecycle';

const lifecycle = new AgentWalletLifecycle();

// Create wallet (triggers automatic funding)
const wallet = await lifecycle.createAgentWallet('agent-002');

// Wait for wallet to become active (funding complete)
let status = wallet.status;
while (status === 'pending') {
  await new Promise((resolve) => setTimeout(resolve, 5000)); // Wait 5 seconds
  const updatedWallet = await lifecycle.getAgentWallet('agent-002');
  status = updatedWallet.status;
}

logger.info('Wallet funding complete', { agentId: 'agent-002', status });
```

### Funding Rate Limits

To prevent abuse, wallet funding is rate-limited:

- **Default limit**: 100 wallet creations per hour
- **Implementation**: Sliding window rate limiter
- **Error response**: `Error: Funding rate limit exceeded`

If you hit the rate limit:

```typescript
try {
  const wallet = await lifecycle.createAgentWallet('agent-003');
} catch (error) {
  if (error.message.includes('rate limit exceeded')) {
    logger.warn('Rate limit hit, waiting before retry');
    await new Promise((resolve) => setTimeout(resolve, 60000)); // Wait 1 minute
    // Retry wallet creation
  }
}
```

### Manual Funding (Advanced)

For custom funding scenarios, use the `AgentWalletFunder`:

```typescript
import { AgentWalletFunder } from '@crosstown/connector/wallet/agent-wallet-funder';

const funder = new AgentWalletFunder();

// Fund specific token on EVM
await funder.fundAgentWallet('agent-001', {
  chain: 'evm',
  token: '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48', // USDC contract
  amount: BigInt(5000000000), // 5000 USDC
});
```

---

## Balance Queries

Track agent wallet balances across EVM tokens.

### Get All Balances

```typescript
import { AgentBalanceTracker } from '@crosstown/connector/wallet/agent-balance-tracker';

const balanceTracker = new AgentBalanceTracker();

// Get all balances for an agent
const balances = await balanceTracker.getAllBalances('agent-001');

// Balances are returned as bigint (wei for ETH, token decimals for ERC-20)
balances.forEach((balance) => {
  logger.info('Balance', {
    chain: balance.chain, // 'evm'
    token: balance.token, // 'ETH', 'USDC address'
    balance: balance.balance.toString(),
    decimals: balance.decimals, // 18 for ETH, 6 for USDC
  });
});
```

### Get Specific Balance

Query balance for a specific chain and token:

```typescript
// Get ETH balance on EVM
const ethBalance = await balanceTracker.getBalance('agent-001', 'evm', 'ETH');
logger.info('ETH balance', { balance: ethBalance.toString() });

// Get USDC balance on EVM (use contract address)
const usdcBalance = await balanceTracker.getBalance(
  'agent-001',
  'evm',
  '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48'
);
logger.info('USDC balance', { balance: usdcBalance.toString() });
```

### Balance Display Example

Display balances with human-readable formatting:

```typescript
import { AgentBalanceTracker } from '@crosstown/connector/wallet/agent-balance-tracker';

async function displayAllBalances(agentId: string) {
  const balanceTracker = new AgentBalanceTracker();
  const balances = await balanceTracker.getAllBalances(agentId);

  // Convert bigint to human-readable format
  const formatBalance = (balance: bigint, decimals: number): string => {
    const divisor = BigInt(10 ** decimals);
    const whole = balance / divisor;
    const fraction = balance % divisor;
    return `${whole}.${fraction.toString().padStart(decimals, '0')}`;
  };

  balances.forEach((b) => {
    const formatted = formatBalance(b.balance, b.decimals);
    logger.info(`${b.chain.toUpperCase()} ${b.token}: ${formatted}`);
  });
}

// Example output:
// EVM ETH: 0.100000000000000000
// EVM 0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48: 1000.000000
```

### Balance Caching and Polling

The balance tracker caches balances and polls on-chain data periodically:

- **Polling interval**: Every 30 seconds (configurable)
- **Cache duration**: Balances cached in memory
- **Automatic refresh**: Background polling updates cached values

```typescript
// Balances are cached - subsequent calls return quickly
const balance1 = await balanceTracker.getBalance('agent-001', 'evm', 'ETH');
const balance2 = await balanceTracker.getBalance('agent-001', 'evm', 'ETH');
// balance2 returns immediately from cache
```

---

## Payment Channel Usage

Payment channels enable fast, low-cost micropayments between agents without on-chain transactions for every payment.

### Opening a Payment Channel

```typescript
import { AgentChannelManager } from '@crosstown/connector/wallet/agent-channel-manager';

const channelManager = new AgentChannelManager();

// Open EVM payment channel
const evmChannelId = await channelManager.openChannel(
  'agent-001', // Your agent ID
  'peer-agent-002', // Peer agent ID
  'evm', // Chain
  'USDC', // Token (or contract address)
  BigInt(1000000000) // Amount: 1000 USDC (6 decimals)
);

logger.info('Channel opened', {
  evmChannelId,
});
```

### Sending Payments Through Channels

Once a channel is open, send instant micropayments:

```typescript
// Send payment through EVM channel
await channelManager.sendPayment(
  'agent-001',
  evmChannelId,
  BigInt(10000000) // Amount: 10 USDC
);

logger.info('Payment sent successfully');
```

### Channel Balance Proofs

Payments include cryptographic balance proofs:

1. **Off-chain updates**: Each payment updates the channel balance off-chain
2. **Balance proofs**: Signed by both parties, provable on-chain
3. **No gas fees**: Payments don't require blockchain transactions
4. **Instant finality**: Sub-second payment confirmation

### Closing Payment Channels

Close channels to settle final balances on-chain:

```typescript
// Close channel and settle final balance
await channelManager.closeChannel('agent-001', evmChannelId);

logger.info('Channel closed and settled', { channelId: evmChannelId });
```

### Listing Agent Channels

Get all channels for an agent:

```typescript
const channels = await channelManager.getAgentChannels('agent-001');

channels.forEach((channel) => {
  logger.info('Channel details', {
    channelId: channel.id,
    peerId: channel.peerId,
    chain: channel.chain,
    token: channel.token,
    balance: channel.balance.toString(),
    status: channel.status, // 'open' | 'closing' | 'closed'
  });
});
```

### Full Channel Lifecycle Example

Complete example showing channel open → payment → close:

```typescript
import { AgentChannelManager } from '@crosstown/connector/wallet/agent-channel-manager';
import { pino } from 'pino';

const logger = pino({ level: 'info' });

async function completeChannelLifecycle() {
  const channelManager = new AgentChannelManager();

  try {
    // 1. Open channel
    logger.info('Opening payment channel...');
    const channelId = await channelManager.openChannel(
      'agent-001',
      'peer-agent-002',
      'evm',
      'USDC',
      BigInt(1000000000) // 1000 USDC
    );
    logger.info('Channel opened', { channelId });

    // 2. Send multiple payments
    logger.info('Sending payments...');
    for (let i = 0; i < 10; i++) {
      await channelManager.sendPayment(
        'agent-001',
        channelId,
        BigInt(10000000) // 10 USDC per payment
      );
      logger.info('Payment sent', { paymentNumber: i + 1 });
    }

    // 3. Close channel
    logger.info('Closing channel...');
    await channelManager.closeChannel('agent-001', channelId);
    logger.info('Channel closed and settled');
  } catch (error) {
    logger.error('Channel lifecycle failed', { error: error.message });
    throw error;
  }
}

completeChannelLifecycle();
```

---

## Integration Patterns

Best practices and patterns for integrating agent wallets into your applications.

### Agent Initialization Flow

Standard initialization pattern for new agents:

```typescript
import { AgentWalletLifecycle } from '@crosstown/connector/wallet/agent-wallet-lifecycle';
import { AgentBalanceTracker } from '@crosstown/connector/wallet/agent-balance-tracker';
import { pino } from 'pino';

const logger = pino({ level: 'info' });

async function initializeAgent(agentId: string) {
  const lifecycle = new AgentWalletLifecycle();
  const balanceTracker = new AgentBalanceTracker();

  try {
    // 1. Check if wallet exists
    let wallet = await lifecycle.getAgentWallet(agentId);

    if (!wallet) {
      // 2. Create new wallet (automatically funded)
      logger.info('Creating new agent wallet', { agentId });
      wallet = await lifecycle.createAgentWallet(agentId);
    }

    // 3. Wait for wallet to become active
    while (wallet.status === 'pending') {
      logger.info('Waiting for wallet activation', { agentId });
      await new Promise((resolve) => setTimeout(resolve, 5000));
      wallet = await lifecycle.getAgentWallet(agentId);
    }

    // 4. Verify balances
    const balances = await balanceTracker.getAllBalances(agentId);
    logger.info('Agent initialized', {
      agentId,
      evmAddress: wallet.evmAddress,
      balanceCount: balances.length,
    });

    return wallet;
  } catch (error) {
    logger.error('Agent initialization failed', { agentId, error: error.message });
    throw error;
  }
}
```

### Wallet Lifecycle Management

Pattern for managing wallet lifecycle in production:

```typescript
import { AgentWalletLifecycle } from '@crosstown/connector/wallet/agent-wallet-lifecycle';

class AgentWalletManager {
  private lifecycle: AgentWalletLifecycle;

  constructor() {
    this.lifecycle = new AgentWalletLifecycle();
  }

  async ensureWalletActive(agentId: string): Promise<void> {
    const wallet = await this.lifecycle.getAgentWallet(agentId);

    if (!wallet) {
      throw new Error(`Wallet not found for agent: ${agentId}`);
    }

    if (wallet.status === 'suspended') {
      // Wallet suspended - investigate reason
      logger.warn('Wallet is suspended', { agentId });
      throw new Error('Wallet suspended - contact support');
    }

    if (wallet.status === 'archived') {
      throw new Error('Wallet archived - create new wallet');
    }

    if (wallet.status !== 'active') {
      throw new Error(`Wallet not active: ${wallet.status}`);
    }
  }

  async suspendAgentWallet(agentId: string, reason: string): Promise<void> {
    logger.warn('Suspending agent wallet', { agentId, reason });
    await this.lifecycle.suspendWallet(agentId, reason);
  }

  async archiveInactiveWallet(agentId: string): Promise<void> {
    logger.info('Archiving inactive wallet', { agentId });
    const archive = await this.lifecycle.archiveWallet(agentId);

    // Store archive metadata for compliance
    logger.info('Wallet archived', {
      agentId,
      archiveId: archive.id,
      archivedAt: archive.archivedAt,
    });
  }
}
```

### Error Handling Patterns

Robust error handling for wallet operations:

```typescript
import { AgentWalletLifecycle } from '@crosstown/connector/wallet/agent-wallet-lifecycle';

async function safeWalletOperation(agentId: string) {
  const lifecycle = new AgentWalletLifecycle();

  try {
    const wallet = await lifecycle.createAgentWallet(agentId);
    logger.info('Wallet created successfully', { agentId });
    return wallet;
  } catch (error) {
    // Categorize error types
    if (error.message.includes('already exists')) {
      logger.warn('Wallet already exists', { agentId });
      // Return existing wallet
      return await lifecycle.getAgentWallet(agentId);
    } else if (error.message.includes('rate limit')) {
      logger.error('Rate limit exceeded', { agentId });
      // Implement exponential backoff
      await new Promise((resolve) => setTimeout(resolve, 60000));
      throw error;
    } else if (error.message.includes('master-seed not found')) {
      logger.error('Master seed not initialized', { agentId });
      // Critical error - requires manual intervention
      throw new Error('System configuration error - contact administrator');
    } else {
      // Unknown error
      logger.error('Wallet operation failed', { agentId, error: error.message });
      throw error;
    }
  }
}
```

### Logging Best Practices

Use structured logging with Pino (never `console.log`):

```typescript
import { pino } from 'pino';

// Good: Structured logging with context
const logger = pino({ level: 'info' });

logger.info('Agent wallet operation', {
  operation: 'createWallet',
  agentId: 'agent-001',
  evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb',
  timestamp: new Date().toISOString(),
});

// Good: Error logging with stack traces
try {
  await lifecycle.createAgentWallet('agent-001');
} catch (error) {
  logger.error('Wallet creation failed', {
    agentId: 'agent-001',
    error: error.message,
    stack: error.stack,
  });
  throw error;
}

// Bad: Never use console.log
// console.log('Agent wallet created:', agentId); // WRONG
```

### Async/Await Pattern

All wallet operations are asynchronous - use `async/await`:

```typescript
// Good: async/await pattern
async function processAgent(agentId: string) {
  const lifecycle = new AgentWalletLifecycle();

  // Sequential operations
  const wallet = await lifecycle.createAgentWallet(agentId);
  const balances = await balanceTracker.getAllBalances(agentId);

  logger.info('Agent processed', { agentId, wallet, balances });
}

// Good: Parallel operations when possible
async function initializeMultipleAgents(agentIds: string[]) {
  const lifecycle = new AgentWalletLifecycle();

  // Create all wallets in parallel
  const wallets = await Promise.all(agentIds.map((id) => lifecycle.createAgentWallet(id)));

  logger.info('All agents initialized', { count: wallets.length });
}

// Bad: Never use callbacks or promise chains
// lifecycle.createAgentWallet('agent-001').then(wallet => { ... }); // WRONG
```

---

## Next Steps

- **Security**: Review [Security Best Practices](../security/agent-wallet-security.md) before production deployment
- **API Reference**: See [Agent Wallet API Documentation](../api/agent-wallet-api.md) for complete API details
- **Troubleshooting**: Check [Troubleshooting Guide](agent-wallet-troubleshooting.md) for common issues
- **FAQ**: Read [Agent Wallet FAQ](agent-wallet-faq.md) for frequently asked questions
- **Production**: Complete [Production Readiness Checklist](../operators/production-readiness-checklist.md) before deployment

---

## Support

For questions or issues:

- **GitHub Issues**: https://github.com/interledger/m2m/issues
- **Documentation**: https://docs.interledger.org/m2m
- **Community**: Interledger community forum
