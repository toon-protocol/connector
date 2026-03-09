# Agent Wallet API Reference

Complete API reference for agent wallet operations, including wallet lifecycle, derivation, balance tracking, channel management, and backup/recovery.

## Table of Contents

1. [AgentWalletLifecycle](#agentwallet lifecycle)
2. [AgentWalletDerivation](#agentwalletderivation)
3. [AgentBalanceTracker](#agentbalancetracker)
4. [AgentChannelManager](#agentchannelmanager)
5. [WalletBackupManager](#walletbackupmanager)
6. [TypeScript Interfaces](#typescript-interfaces)
7. [Error Codes](#error-codes)

---

## AgentWalletLifecycle

Manages the complete lifecycle of agent wallets from creation to archival.

**Import:**

```typescript
import { AgentWalletLifecycle } from '@crosstown/connector/wallet/agent-wallet-lifecycle';
```

### Methods

#### `createAgentWallet(agentId: string): Promise<WalletLifecycleRecord>`

Creates a new agent wallet with a unique EVM address. Automatically triggers funding process.

**Parameters:**

- `agentId` (string, required): Unique identifier for the agent

**Returns:**

- `Promise<WalletLifecycleRecord>`: Wallet lifecycle record containing addresses and metadata

**Throws:**

- `Error('Wallet already exists for agent')`: If agent already has a wallet
- `Error('master-seed not found in storage')`: If master seed not initialized
- `Error('Funding rate limit exceeded')`: If rate limit exceeded (100 wallets/hour)

**Example:**

```typescript
const lifecycle = new AgentWalletLifecycle();

try {
  const wallet = await lifecycle.createAgentWallet('agent-001');
  logger.info('Wallet created', {
    agentId: wallet.agentId,
    evmAddress: wallet.evmAddress,
    status: wallet.status,
  });
} catch (error) {
  logger.error('Wallet creation failed', { error: error.message });
  throw error;
}
```

**Response Structure:**

```typescript
{
  agentId: 'agent-001',
  evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb',
  derivationIndex: 0,
  createdAt: Date,
  status: 'pending', // Will transition to 'active' after funding
  fundingTransactions: []
}
```

---

#### `getAgentWallet(agentId: string): Promise<AgentWallet | null>`

Retrieves an existing agent wallet.

**Parameters:**

- `agentId` (string, required): Agent identifier

**Returns:**

- `Promise<AgentWallet | null>`: Wallet if exists, null otherwise

**Throws:**

- None (returns null for non-existent wallets)

**Example:**

```typescript
const wallet = await lifecycle.getAgentWallet('agent-001');

if (!wallet) {
  logger.warn('Wallet not found', { agentId: 'agent-001' });
  // Create new wallet
  const newWallet = await lifecycle.createAgentWallet('agent-001');
} else {
  logger.info('Wallet found', { agentId: wallet.agentId, status: wallet.status });
}
```

---

#### `suspendWallet(agentId: string, reason: string): Promise<void>`

Temporarily suspends a wallet, preventing transactions. Wallet can be reactivated later.

**Parameters:**

- `agentId` (string, required): Agent identifier
- `reason` (string, required): Reason for suspension (for audit trail)

**Returns:**

- `Promise<void>`

**Throws:**

- `Error('Wallet not found')`: If wallet doesn't exist
- `Error('Wallet already suspended')`: If wallet already suspended

**Example:**

```typescript
await lifecycle.suspendWallet('agent-001', 'Security review pending');

logger.info('Wallet suspended', {
  agentId: 'agent-001',
  reason: 'Security review pending',
});
```

**Use Cases:**

- Security investigation
- Compliance review
- Temporary agent deactivation
- Fraud prevention

---

#### `archiveWallet(agentId: string): Promise<WalletArchive>`

Permanently archives a wallet after inactivity. Cannot be reactivated (create new wallet instead).

**Parameters:**

- `agentId` (string, required): Agent identifier

**Returns:**

- `Promise<WalletArchive>`: Archive record with final balances and metadata

**Throws:**

- `Error('Wallet not found')`: If wallet doesn't exist
- `Error('Cannot archive active wallet')`: If wallet has recent activity

**Example:**

```typescript
const archive = await lifecycle.archiveWallet('agent-001');

logger.info('Wallet archived', {
  agentId: archive.agentId,
  archiveId: archive.id,
  finalBalances: archive.finalBalances,
  archivedAt: archive.archivedAt,
});
```

**Archive Record Structure:**

```typescript
{
  id: 'archive-001',
  agentId: 'agent-001',
  evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb',
  finalBalances: [
    { chain: 'evm', token: 'ETH', balance: '0' },
    { chain: 'evm', token: 'USDC', balance: '1000000' }
  ],
  archivedAt: Date,
  reason: 'Inactivity > 90 days'
}
```

---

## AgentWalletDerivation

Derives agent wallets from HD master seed using BIP-32/BIP-44 standards.

**Import:**

```typescript
import { AgentWalletDerivation } from '@crosstown/connector/wallet/agent-wallet-derivation';
```

### Methods

#### `deriveAgentWallet(agentId: string): Promise<AgentWallet>`

Derives a new wallet for an agent using HD derivation.

**Parameters:**

- `agentId` (string, required): Unique agent identifier

**Returns:**

- `Promise<AgentWallet>`: Derived wallet with EVM address

**Throws:**

- `Error('Wallet already exists for agent')`: If agent already has a wallet
- `Error('master-seed not found')`: If master seed not initialized
- `Error('Invalid derivation index')`: If derivation index out of range (0 to 2^31-1)

**Derivation Path:**

- EVM: `m/44'/60'/1'/0/{agentIndex}`

**Example:**

```typescript
const derivation = new AgentWalletDerivation();

const wallet = await derivation.deriveAgentWallet('agent-001');

logger.info('Wallet derived', {
  agentId: wallet.agentId,
  evmAddress: wallet.evmAddress,
  derivationIndex: wallet.derivationIndex,
});
```

---

#### `getAgentWallet(agentId: string): Promise<AgentWallet | null>`

Retrieves an existing derived wallet.

**Parameters:**

- `agentId` (string, required): Agent identifier

**Returns:**

- `Promise<AgentWallet | null>`: Wallet if exists, null otherwise

**Example:**

```typescript
const wallet = await derivation.getAgentWallet('agent-001');

if (wallet) {
  logger.info('Wallet exists', {
    evmAddress: wallet.evmAddress,
  });
}
```

---

#### `getAgentSigner(agentId: string): Promise<Wallet>`

Gets a signer instance for EVM transaction signing.

**Parameters:**

- `agentId` (string, required): Agent identifier

**Returns:**

- `Promise<Wallet>`: ethers.js Wallet instance

**Throws:**

- `Error('Wallet not found')`: If agent wallet doesn't exist

**Example:**

```typescript
// Get EVM signer for transaction signing
const evmSigner = await derivation.getAgentSigner('agent-001');
const tx = await evmSigner.sendTransaction({
  to: '0x...',
  value: ethers.parseEther('0.1'),
});
```

**Security Note:** Signers provide access to private keys. Never expose signers in API responses or logs.

---

#### `batchDeriveWallets(agentIds: string[]): Promise<AgentWallet[]>`

Efficiently derives wallets for multiple agents in batch.

**Parameters:**

- `agentIds` (string[], required): Array of agent identifiers

**Returns:**

- `Promise<AgentWallet[]>`: Array of derived wallets

**Throws:**

- `Error('Batch size exceeds maximum')`: If batch > 1000 agents
- `Error('Duplicate agent IDs in batch')`: If array contains duplicates

**Example:**

```typescript
const agentIds = ['agent-001', 'agent-002', 'agent-003'];
const wallets = await derivation.batchDeriveWallets(agentIds);

logger.info('Batch derivation complete', {
  count: wallets.length,
  wallets: wallets.map((w) => ({
    agentId: w.agentId,
    evmAddress: w.evmAddress,
  })),
});
```

---

## AgentBalanceTracker

Tracks agent wallet balances across multiple chains and tokens.

**Import:**

```typescript
import { AgentBalanceTracker } from '@crosstown/connector/wallet/agent-balance-tracker';
```

### Methods

#### `getBalance(agentId: string, token: string): Promise<bigint>`

Gets the current balance for a specific token.

**Parameters:**

- `agentId` (string, required): Agent identifier
- `token` (string, required): Token identifier
  - `'ETH'` or ERC20 contract address (e.g., `'0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48'` for USDC)

**Returns:**

- `Promise<bigint>`: Balance in smallest unit (wei for ETH, token decimals for ERC20)

**Throws:**

- `Error('Wallet not found')`: If agent wallet doesn't exist
- `Error('Unsupported token')`: If token not configured

**Example:**

```typescript
const balanceTracker = new AgentBalanceTracker();

// Get ETH balance (18 decimals)
const ethBalance = await balanceTracker.getBalance('agent-001', 'ETH');
logger.info('ETH balance', { balance: ethBalance.toString() }); // '100000000000000000' = 0.1 ETH

// Get USDC balance (6 decimals)
const usdcBalance = await balanceTracker.getBalance(
  'agent-001',
  '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48'
);
logger.info('USDC balance', { balance: usdcBalance.toString() }); // '1000000000' = 1000 USDC
```

---

#### `getAllBalances(agentId: string): Promise<AgentBalance[]>`

Gets all tracked balances for an agent across all chains and tokens.

**Parameters:**

- `agentId` (string, required): Agent identifier

**Returns:**

- `Promise<AgentBalance[]>`: Array of balance objects

**Throws:**

- `Error('Wallet not found')`: If agent wallet doesn't exist

**Example:**

```typescript
const balances = await balanceTracker.getAllBalances('agent-001');

balances.forEach((balance) => {
  logger.info('Balance', {
    chain: balance.chain,
    token: balance.token,
    balance: balance.balance.toString(),
    decimals: balance.decimals,
    lastUpdated: balance.lastUpdated,
  });
});
```

**Response Structure:**

```typescript
[
  {
    agentId: 'agent-001',
    chain: 'evm',
    token: 'ETH',
    balance: 100000000000000000n, // bigint
    decimals: 18,
    lastUpdated: Date,
  },
  {
    agentId: 'agent-001',
    chain: 'evm',
    token: '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48',
    balance: 1000000000n,
    decimals: 6,
    lastUpdated: Date,
  },
];
```

**Caching:** Balances are cached and polled every 30 seconds (configurable). Subsequent calls return cached values for performance.

---

## AgentChannelManager

Manages payment channels for instant micropayments between agents.

**Import:**

```typescript
import { AgentChannelManager } from '@crosstown/connector/wallet/agent-channel-manager';
```

### Methods

#### `openChannel(agentId: string, peerId: string, token: string, amount: bigint): Promise<string>`

Opens a new payment channel with a peer agent on Base L2.

**Parameters:**

- `agentId` (string, required): Your agent identifier
- `peerId` (string, required): Peer agent identifier
- `token` (string, required): Token for channel payments (`'ETH'`, `'USDC'`, or ERC20 contract address)
- `amount` (bigint, required): Channel funding amount (in smallest unit)

**Returns:**

- `Promise<string>`: Channel ID for reference in subsequent operations

**Throws:**

- `Error('Wallet not found')`: If agent wallet doesn't exist
- `Error('Insufficient balance')`: If agent lacks funds for channel
- `Error('Invalid peer ID')`: If peer agent doesn't exist
- `Error('Channel already exists')`: If channel already open with peer

**Example:**

```typescript
const channelManager = new AgentChannelManager();

// Open EVM channel with 1000 USDC
const channelId = await channelManager.openChannel(
  'agent-001',
  'peer-agent-002',
  'evm',
  'USDC',
  BigInt(1000000000) // 1000 USDC (6 decimals)
);

logger.info('Channel opened', {
  channelId,
  agentId: 'agent-001',
  peerId: 'peer-agent-002',
  chain: 'evm',
  token: 'USDC',
  amount: '1000000000',
});
```

**Channel Opening Process:**

1. Validate agent and peer wallets exist
2. Check sufficient balance for channel funding
3. Create on-chain channel transaction
4. Wait for blockchain confirmation
5. Return channel ID for future operations

---

#### `sendPayment(agentId: string, channelId: string, amount: bigint): Promise<void>`

Sends an instant off-chain payment through an open channel.

**Parameters:**

- `agentId` (string, required): Your agent identifier
- `channelId` (string, required): Channel ID (from `openChannel()`)
- `amount` (bigint, required): Payment amount (in smallest unit)

**Returns:**

- `Promise<void>`

**Throws:**

- `Error('Channel not found')`: If channel doesn't exist
- `Error('Channel not open')`: If channel is closed or closing
- `Error('Insufficient channel balance')`: If payment exceeds remaining channel balance
- `Error('Invalid amount')`: If amount <= 0

**Example:**

```typescript
// Send 10 USDC through channel
await channelManager.sendPayment(
  'agent-001',
  'channel-evm-001',
  BigInt(10000000) // 10 USDC
);

logger.info('Payment sent', {
  agentId: 'agent-001',
  channelId: 'channel-evm-001',
  amount: '10000000',
});
```

**Payment Process:**

1. Validate channel is open
2. Check sufficient channel balance
3. Create signed balance proof
4. Update off-chain channel state
5. Send balance proof to peer
6. Return immediately (no blockchain transaction)

**Performance:** Payments are instant (sub-second) with no gas fees.

---

#### `closeChannel(agentId: string, channelId: string): Promise<void>`

Closes a payment channel and settles final balance on-chain.

**Parameters:**

- `agentId` (string, required): Your agent identifier
- `channelId` (string, required): Channel ID to close

**Returns:**

- `Promise<void>`

**Throws:**

- `Error('Channel not found')`: If channel doesn't exist
- `Error('Channel already closed')`: If channel already closed

**Example:**

```typescript
await channelManager.closeChannel('agent-001', 'channel-evm-001');

logger.info('Channel closed', {
  agentId: 'agent-001',
  channelId: 'channel-evm-001',
});
```

**Channel Closing Process:**

1. Submit final balance proof to blockchain
2. Wait for settlement transaction confirmation
3. Distribute final balances to agents
4. Update channel status to 'closed'

**Settlement Time:**

- EVM: ~15 seconds (Base L2 block time)

---

#### `getAgentChannels(agentId: string): Promise<AgentChannel[]>`

Lists all payment channels for an agent.

**Parameters:**

- `agentId` (string, required): Agent identifier

**Returns:**

- `Promise<AgentChannel[]>`: Array of agent channels

**Throws:**

- `Error('Wallet not found')`: If agent wallet doesn't exist

**Example:**

```typescript
const channels = await channelManager.getAgentChannels('agent-001');

channels.forEach((channel) => {
  logger.info('Channel', {
    channelId: channel.id,
    peerId: channel.peerId,
    chain: channel.chain,
    token: channel.token,
    balance: channel.balance.toString(),
    status: channel.status,
  });
});
```

**Response Structure:**

```typescript
[
  {
    id: 'channel-evm-001',
    agentId: 'agent-001',
    peerId: 'peer-agent-002',
    chain: 'evm',
    token: 'USDC',
    balance: 900000000n, // Remaining: 900 USDC
    initialAmount: 1000000000n, // Started with: 1000 USDC
    paymentsCount: 10,
    status: 'open', // 'open' | 'closing' | 'closed'
    openedAt: Date,
    lastPaymentAt: Date,
  },
];
```

---

## WalletBackupManager

Creates and restores encrypted backups of agent wallets and master seed.

**Import:**

```typescript
import { WalletBackupManager } from '@crosstown/connector/wallet/wallet-backup-manager';
```

### Methods

#### `createFullBackup(password: string): Promise<WalletBackup>`

Creates a full encrypted backup of master seed and all agent wallet metadata.

**Parameters:**

- `password` (string, required): Strong password for backup encryption (16+ characters recommended)

**Returns:**

- `Promise<WalletBackup>`: Encrypted backup object

**Throws:**

- `Error('master-seed not found')`: If master seed not initialized
- `Error('Weak password')`: If password < 12 characters

**Example:**

```typescript
const backupManager = new WalletBackupManager();

const backup = await backupManager.createFullBackup('strong-password-123456789');

logger.info('Backup created', {
  backupId: backup.id,
  walletCount: backup.wallets.length,
  timestamp: backup.createdAt,
});

// Save backup to secure location (NOT version control!)
import { writeFile } from 'fs/promises';
await writeFile('backup-2026-01-21.enc', JSON.stringify(backup));
```

**Backup Structure:**

```typescript
{
  id: 'backup-20260121-123456',
  version: '1.0',
  createdAt: Date,
  encryptedSeed: 'AES-256-GCM encrypted master seed',
  encryptionSalt: 'Unique salt for key derivation',
  wallets: [
    {
      agentId: 'agent-001',
      evmAddress: '0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb',
      derivationIndex: 0,
      createdAt: Date,
      lastActive: Date,
      balancesSnapshot: [...]
    }
  ]
}
```

**Security:**

- Backup is encrypted with AES-256-GCM
- Password used for key derivation (PBKDF2, 100,000 rounds)
- Never store backup password in code or version control

---

#### `restoreFromBackup(backupData: WalletBackup, password: string): Promise<void>`

Restores agent wallets from an encrypted backup.

**Parameters:**

- `backupData` (WalletBackup, required): Backup object (from `createFullBackup()`)
- `password` (string, required): Password used during backup creation

**Returns:**

- `Promise<void>`

**Throws:**

- `Error('Invalid password')`: If password incorrect
- `Error('Corrupt backup data')`: If backup file tampered with
- `Error('Master seed already exists')`: If system already initialized

**Example:**

```typescript
import { readFile } from 'fs/promises';

// Load backup from secure storage
const backupJson = await readFile('backup-2026-01-21.enc', 'utf-8');
const backupData = JSON.parse(backupJson);

// Restore backup
await backupManager.restoreFromBackup(backupData, 'strong-password-123456789');

logger.info('Backup restored', {
  backupId: backupData.id,
  walletsRestored: backupData.wallets.length,
});

// Verify restored wallets
const lifecycle = new AgentWalletLifecycle();
const wallet = await lifecycle.getAgentWallet('agent-001');
logger.info('Wallet verified', { agentId: wallet.agentId });
```

**Restore Process:**

1. Decrypt master seed using password
2. Re-derive all agent wallet addresses
3. Verify addresses match backup metadata
4. Reconcile balances with on-chain data
5. Restore wallet lifecycle states

**Balance Reconciliation:** Restored balances are reconciled with current on-chain balances automatically.

---

## TypeScript Interfaces

Core type definitions for agent wallet APIs.

### AgentWallet

```typescript
interface AgentWallet {
  agentId: string; // Unique agent identifier
  evmAddress: string; // EVM address (0x-prefixed, 42 chars)
  derivationIndex: number; // HD wallet derivation index (0 to 2^31-1)
  createdAt: Date; // Wallet creation timestamp
  status: WalletStatus; // Wallet lifecycle status
}

type WalletStatus = 'pending' | 'active' | 'suspended' | 'archived';
```

### WalletLifecycleRecord

```typescript
interface WalletLifecycleRecord extends AgentWallet {
  fundingTransactions: FundingTransaction[]; // Automatic funding tx records
  suspensionReason?: string; // Reason if suspended
  suspendedAt?: Date; // Suspension timestamp
  lastActive?: Date; // Last transaction timestamp
}

interface FundingTransaction {
  chain: 'evm';
  token: string;
  amount: bigint;
  txHash: string;
  status: 'pending' | 'confirmed' | 'failed';
}
```

### AgentBalance

```typescript
interface AgentBalance {
  agentId: string;
  chain: 'evm';
  token: string; // 'ETH' or ERC20 address
  balance: bigint; // Balance in smallest unit
  decimals: number; // Token decimals (18 for ETH, 6 for USDC)
  lastUpdated: Date; // Last balance update timestamp
}
```

### AgentChannel

```typescript
interface AgentChannel {
  id: string; // Unique channel identifier
  agentId: string; // Your agent ID
  peerId: string; // Peer agent ID
  chain: 'evm'; // Blockchain chain (Base L2)
  token: string; // Token for payments
  balance: bigint; // Remaining channel balance
  initialAmount: bigint; // Initial channel funding
  paymentsCount: number; // Total payments sent through channel
  status: ChannelStatus; // Channel status
  openedAt: Date; // Channel open timestamp
  lastPaymentAt?: Date; // Last payment timestamp
  closedAt?: Date; // Channel close timestamp
}

type ChannelStatus = 'open' | 'closing' | 'closed';
```

### WalletArchive

```typescript
interface WalletArchive {
  id: string; // Archive record ID
  agentId: string;
  evmAddress: string;
  finalBalances: AgentBalance[]; // Balances at archival time
  archivedAt: Date;
  reason: string; // Archival reason (e.g., 'Inactivity')
}
```

### WalletBackup

```typescript
interface WalletBackup {
  id: string; // Backup ID
  version: string; // Backup format version
  createdAt: Date;
  encryptedSeed: string; // AES-256-GCM encrypted master seed
  encryptionSalt: string; // Salt for key derivation
  wallets: WalletBackupEntry[]; // All agent wallets metadata
}

interface WalletBackupEntry {
  agentId: string;
  evmAddress: string;
  derivationIndex: number;
  createdAt: Date;
  lastActive?: Date;
  balancesSnapshot: AgentBalance[];
}
```

---

## Error Codes

Standard error messages and meanings.

### Wallet Lifecycle Errors

| Error Message                      | Meaning                            | Resolution                                          |
| ---------------------------------- | ---------------------------------- | --------------------------------------------------- |
| `Wallet already exists for agent`  | Agent ID already has a wallet      | Use `getAgentWallet()` to retrieve existing wallet  |
| `Wallet not found`                 | Agent wallet doesn't exist         | Create new wallet with `createAgentWallet()`        |
| `master-seed not found in storage` | Master seed not initialized        | Initialize seed manager with `generateMasterSeed()` |
| `Funding rate limit exceeded`      | Too many wallet creations per hour | Wait for rate limit window to reset                 |
| `Wallet already suspended`         | Wallet is already suspended        | Check suspension reason, reactivate if needed       |
| `Cannot archive active wallet`     | Wallet has recent activity         | Wait for inactivity period or suspend first         |

### Balance Tracking Errors

| Error Message            | Meaning                          | Resolution                                        |
| ------------------------ | -------------------------------- | ------------------------------------------------- |
| `Invalid chain`          | Chain parameter not 'evm'        | Use valid chain identifier                        |
| `Unsupported token`      | Token not configured in system   | Add token configuration or use supported token    |
| `Balance polling failed` | Unable to query on-chain balance | Check RPC endpoint configuration and connectivity |

### Channel Management Errors

| Error Message                  | Meaning                            | Resolution                                     |
| ------------------------------ | ---------------------------------- | ---------------------------------------------- |
| `Channel not found`            | Channel ID doesn't exist           | Verify channel ID or open new channel          |
| `Channel not open`             | Channel is closed or closing       | Open new channel for payments                  |
| `Channel already exists`       | Channel already open with peer     | Use existing channel or close and reopen       |
| `Insufficient channel balance` | Payment exceeds remaining balance  | Close channel and open new one with more funds |
| `Insufficient balance`         | Agent lacks funds for channel      | Fund agent wallet before opening channel       |
| `Invalid peer ID`              | Peer agent doesn't exist           | Verify peer agent ID or create peer wallet     |
| `Channel already closed`       | Attempting to close closed channel | Channel already settled                        |

### Backup/Recovery Errors

| Error Message                | Meaning                                | Resolution                                         |
| ---------------------------- | -------------------------------------- | -------------------------------------------------- |
| `Invalid password`           | Backup password incorrect              | Use correct password from backup creation          |
| `Weak password`              | Password < 12 characters               | Use stronger password (16+ characters recommended) |
| `Corrupt backup data`        | Backup file tampered with or corrupted | Use a valid backup file                            |
| `Master seed already exists` | System already initialized             | Clear existing data before restore (use caution!)  |

---

## Rate Limits

API rate limits to prevent abuse:

| Operation             | Limit         | Window   | Error Response                         |
| --------------------- | ------------- | -------- | -------------------------------------- |
| `createAgentWallet()` | 100 wallets   | 1 hour   | `Funding rate limit exceeded`          |
| `openChannel()`       | 50 channels   | 1 hour   | `Channel creation rate limit exceeded` |
| `sendPayment()`       | 1000 payments | 1 minute | `Payment rate limit exceeded`          |

**Rate Limit Configuration:**
Rate limits are configurable via environment variables:

```bash
WALLET_CREATION_RATE_LIMIT=100
WALLET_CREATION_RATE_WINDOW=3600  # seconds
CHANNEL_CREATION_RATE_LIMIT=50
PAYMENT_RATE_LIMIT=1000
```

---

## Next Steps

- **Integration Guide**: See [Agent Wallet Integration Guide](../guides/agent-wallet-integration.md)
- **Security**: Review [Security Best Practices](../security/agent-wallet-security.md)
- **Troubleshooting**: Check [Troubleshooting Guide](../guides/agent-wallet-troubleshooting.md)
- **Code Examples**: View [Multi-Language Examples](../../examples/)

---

## API Versioning

Current API version: **v1.0**

**Breaking Changes Policy:**

- Major version changes indicate breaking API changes
- Minor version changes add backward-compatible features
- Patch version changes for bug fixes only

**Deprecation Policy:**

- Deprecated APIs supported for minimum 6 months
- Deprecation notices included in API responses
- Migration guides provided for deprecated APIs
