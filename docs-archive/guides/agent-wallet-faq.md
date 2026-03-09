# Agent Wallet FAQ

Frequently asked questions about agent wallet integration, functionality, and best practices.

## Table of Contents

1. [Wallet Lifecycle](#wallet-lifecycle)
2. [Backup and Recovery](#backup-and-recovery)
3. [Multi-Chain Support](#multi-chain-support)
4. [Performance and Scalability](#performance-and-scalability)
5. [Security](#security)
6. [Payment Channels](#payment-channels)
7. [Integration and Development](#integration-and-development)
8. [Troubleshooting](#troubleshooting)

---

## Wallet Lifecycle

### Q: How are agent wallets created?

**A:** Agent wallets are created via `AgentWalletLifecycle.createAgentWallet(agentId)`, which derives a unique EVM address from a master seed using BIP-32/BIP-44 HD wallet standards.

Each agent receives:

- One EVM address (Base L2): `m/44'/60'/1'/0/{agentIndex}`

The master seed generates up to 2^31 (2.1 billion) unique agent wallets.

**Example:**

```typescript
import { AgentWalletLifecycle } from '@crosstown/connector/wallet/agent-wallet-lifecycle';

const lifecycle = new AgentWalletLifecycle();
const wallet = await lifecycle.createAgentWallet('agent-001');

// wallet.evmAddress: "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb"
```

**Reference:** [Integration Guide - Wallet Creation](agent-wallet-integration.md#wallet-creation)

---

### Q: What wallet states exist?

**A:** Agent wallets transition through four states:

1. **Pending**: Wallet created but not yet funded (initial state)
2. **Active**: Wallet funded and ready for transactions
3. **Suspended**: Temporarily disabled (fraud, security review, manual suspension)
4. **Archived**: Permanently archived after inactivity (cannot be reactivated)

**State Transitions:**

```
pending → active → suspended → active (can be reactivated)
                 ↓
              archived (permanent)
```

**Checking Wallet State:**

```typescript
const wallet = await lifecycle.getAgentWallet('agent-001');

if (wallet.status === 'suspended') {
  logger.warn('Wallet is suspended', { reason: wallet.suspensionReason });
} else if (wallet.status === 'active') {
  logger.info('Wallet is ready for transactions');
}
```

**Reference:** [Integration Guide - Wallet Lifecycle States](agent-wallet-integration.md#wallet-lifecycle-states)

---

### Q: How do I suspend a wallet?

**A:** Call `suspendWallet(agentId, reason)` to temporarily disable a wallet. The wallet can be reactivated later.

**Use Cases:**

- Security investigation
- Fraud detection
- Compliance review
- Temporary agent deactivation

**Example:**

```typescript
// Suspend wallet
await lifecycle.suspendWallet('agent-001', 'Security review pending');

logger.info('Wallet suspended');

// Later: Reactivate wallet
await lifecycle.reactivateWallet('agent-001');

logger.info('Wallet reactivated');
```

**Note:** Suspended wallets cannot perform any transactions until reactivated.

**Reference:** [API Reference - suspendWallet()](../api/agent-wallet-api.md#suspendwallet)

---

### Q: When are wallets auto-archived?

**A:** Wallets are automatically archived after a configurable period of inactivity (default: 90 days).

**Archival Criteria:**

- No transactions in last 90 days
- No payment channel activity
- No funding requests
- Wallet status is 'active' (not already suspended/archived)

**Configuration:**

```typescript
const archivalConfig = {
  inactivityPeriodDays: 90, // Days of inactivity before archival
  enableAutoArchival: true, // Enable/disable auto-archival
};
```

**Manual Archival:**

```typescript
// Archive inactive wallet manually
const archive = await lifecycle.archiveWallet('agent-001');

logger.info('Wallet archived', {
  archiveId: archive.id,
  finalBalances: archive.finalBalances,
  archivedAt: archive.archivedAt,
});
```

**Important:** Archived wallets **cannot be reactivated**. Create a new wallet instead.

**Reference:** [API Reference - archiveWallet()](../api/agent-wallet-api.md#archivewallet)

---

## Backup and Recovery

### Q: How do I backup agent wallets?

**A:** Use `WalletBackupManager.createFullBackup(password)` to create an encrypted backup of the master seed and all agent wallet metadata.

**What's Included in Backup:**

- Encrypted master seed (BIP-39 mnemonic)
- All agent wallet metadata (addresses, derivation indices)
- Balance snapshots at backup time
- Wallet lifecycle states

**Example:**

```typescript
import { WalletBackupManager } from '@crosstown/connector/wallet/wallet-backup-manager';

const backupManager = new WalletBackupManager();

// Create encrypted backup
const backup = await backupManager.createFullBackup('strong-password-123456789');

logger.info('Backup created', {
  backupId: backup.id,
  walletCount: backup.wallets.length,
  timestamp: backup.createdAt,
});

// Save to secure location (NOT version control!)
import { writeFile } from 'fs/promises';
await writeFile(`/secure/backups/backup-${backup.id}.enc`, JSON.stringify(backup));
```

**Encryption:** AES-256-GCM with PBKDF2 key derivation (100,000 rounds)

**Reference:** [API Reference - WalletBackupManager](../api/agent-wallet-api.md#walletbackupmanager)

---

### Q: Can I restore wallets on a new server?

**A:** Yes! Use `restoreFromBackup(backupData, password)` to restore all agent wallets on a new server.

**Restore Process:**

1. Load backup file from secure storage
2. Decrypt master seed using password
3. Re-derive all agent wallet addresses
4. Verify addresses match backup metadata
5. Reconcile balances with current on-chain data

**Example:**

```typescript
import { readFile } from 'fs/promises';

// Load backup from secure storage
const backupJson = await readFile('/secure/backups/backup-latest.enc', 'utf-8');
const backup = JSON.parse(backupJson);

// Restore on new server
await backupManager.restoreFromBackup(backup, 'strong-password-123456789');

logger.info('Backup restored successfully', {
  backupId: backup.id,
  walletsRestored: backup.wallets.length,
});

// Verify restoration
const wallet = await lifecycle.getAgentWallet('agent-001');
logger.info('Wallet verified', { agentId: wallet.agentId });
```

**Automatic Balance Reconciliation:** Restored balances are automatically reconciled with current on-chain balances.

**Reference:** [API Reference - restoreFromBackup()](../api/agent-wallet-api.md#restorefrombackup)

---

### Q: How often should I backup?

**A:** Follow this backup schedule for production:

**Recommended Schedule:**

- **Daily**: Incremental backups (metadata only)
- **Weekly**: Full backups (master seed + metadata)
- **Monthly**: Off-site backups (stored offline in safe)

**Backup Strategy:**

```typescript
// Daily incremental backup (fast)
const incrementalBackup = await backupManager.createIncrementalBackup();
await saveToLocal(incrementalBackup);

// Weekly full backup (comprehensive)
const fullBackup = await backupManager.createFullBackup(password);
await saveToLocal(fullBackup);
await saveToCloud(fullBackup); // S3, GCS, etc.

// Monthly off-site backup (disaster recovery)
const monthlyBackup = await backupManager.createFullBackup(password);
await saveToOfflineMedia(monthlyBackup); // USB, offline storage
```

**Automation:**

```bash
# Cron schedule
0 2 * * * /usr/local/bin/wallet-backup daily     # 2 AM daily
0 3 * * 0 /usr/local/bin/wallet-backup weekly    # 3 AM Sunday
0 4 1 * * /usr/local/bin/wallet-backup monthly   # 4 AM 1st of month
```

**Reference:** [Security Best Practices - Backup Security](../security/agent-wallet-security.md#backup-security)

---

### Q: What happens if I lose the master seed?

**A:** **All agent wallets are permanently lost** if you lose the master seed without a backup.

**Impact:**

- Cannot derive any agent wallet private keys
- Cannot access agent funds
- Cannot recover wallet addresses
- All agent wallets must be recreated with new master seed

**Recovery Options:**

1. **If you have a backup**: Restore from most recent backup
2. **If you have the mnemonic phrase**: Import mnemonic and regenerate seed
3. **If you have neither**: **No recovery possible** - funds are lost

**Prevention (Critical):**

- ✅ Create encrypted backups immediately after seed generation
- ✅ Store backups in multiple secure locations
- ✅ Test backup restore regularly (quarterly)
- ✅ Use enterprise password manager for backup passwords
- ❌ **NEVER** commit master seed to version control
- ❌ **NEVER** store master seed in plaintext

**Reference:** [Troubleshooting - Master Seed Issues](agent-wallet-troubleshooting.md#master-seed-issues)

---

## EVM Wallet Details

### Q: What format are agent wallet addresses?

**A:** Each agent receives an EVM address on Base L2:

**EVM (Base L2):**

- Format: 0x-prefixed hex (42 characters)
- Derivation path: `m/44'/60'/1'/0/{agentIndex}`
- Example: `0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb`

**Addresses are derived from the agent index**, ensuring deterministic recovery.

**Example:**

```typescript
const wallet = await lifecycle.createAgentWallet('agent-001');

logger.info('Wallet created', {
  agentId: wallet.agentId,
  evmAddress: wallet.evmAddress,
  derivationIndex: wallet.derivationIndex,
});
```

**Reference:** [Integration Guide - Address Output](agent-wallet-integration.md#multi-chain-address-output)

---

### Q: How do agents open payment channels?

**A:** Use the AgentChannelManager to open EVM payment channels:

**Example:**

```typescript
import { AgentChannelManager } from '@crosstown/connector/wallet/agent-channel-manager';

const channelManager = new AgentChannelManager();

// Open EVM payment channel on Base L2
const channelId = await channelManager.openChannel(
  'agent-001',
  'peer-agent-002',
  'evm',
  'USDC',
  BigInt(1000000000)
);
```

**Reference:** [API Reference - openChannel()](../api/agent-wallet-api.md#openchannel)

---

### Q: What tokens are supported?

**A:** The system supports native tokens and ERC20 tokens on EVM (Base L2):

- **Native**: ETH (for gas fees)
- **ERC20**: USDC, DAI, USDT, and any ERC20-compliant token
- **Custom**: Add token by configuring contract address

**Token Configuration:**

```typescript
// Check supported tokens
const balances = await balanceTracker.getAllBalances('agent-001');

balances.forEach((b) => {
  logger.info('Supported token', {
    chain: b.chain,
    token: b.token,
    decimals: b.decimals,
  });
});

// Output:
// { chain: 'evm', token: 'ETH', decimals: 18 }
// { chain: 'evm', token: '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48', decimals: 6 }
```

**Reference:** [Integration Guide - Balance Queries](agent-wallet-integration.md#balance-queries)

---

### Q: How do I add a new ERC20 token?

**A:** Configure the token in the balance tracker with its contract address:

**Example:**

```typescript
import { AgentBalanceTracker } from '@crosstown/connector/wallet/agent-balance-tracker';

const balanceTracker = new AgentBalanceTracker({
  tokens: {
    evm: [
      { symbol: 'ETH', address: 'native', decimals: 18 },
      { symbol: 'USDC', address: '0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48', decimals: 6 },
      { symbol: 'DAI', address: '0x6B175474E89094C44Da98b954EedeAC495271d0F', decimals: 18 },
      // Add custom token
      { symbol: 'CUSTOM', address: '0x1234567890abcdef...', decimals: 18 },
    ],
  },
});

// Query custom token balance
const customBalance = await balanceTracker.getBalance('agent-001', 'evm', '0x1234567890abcdef...');

logger.info('Custom token balance', { balance: customBalance.toString() });
```

**Token Requirements:**

- Must be ERC20-compliant
- Contract must be deployed on Base L2
- Token decimals must be specified correctly

**Reference:** [API Reference - AgentBalanceTracker](../api/agent-wallet-api.md#agentbalancetracker)

---

## Performance and Scalability

### Q: How many agents can I support?

**A:** Up to **2^31 agents (2.1 billion)** from a single master seed.

**Technical Details:**

- Derivation index range: 0 to 2,147,483,647
- Each agent uses one index
- Addresses are deterministically derived
- No practical limit for most applications

**Scalability Benchmarks:**

- **Wallet Creation**: 1000 wallets/hour (rate limited)
- **Balance Queries**: 10,000 queries/second (cached)
- **Payment Channels**: 50 channels/agent (recommended)
- **Storage**: ~1KB per agent wallet (metadata)

**Example:**

```typescript
// High-scale deployment
const agentCount = 1000000; // 1 million agents

// Batch creation (respects rate limits)
for (let i = 0; i < agentCount; i += 100) {
  const batch = Array.from({ length: 100 }, (_, j) => `agent-${i + j}`);

  await Promise.all(batch.map((id) => lifecycle.createAgentWallet(id)));

  // Rate limit: 100/hour
  if (i % 100 === 0 && i < agentCount - 100) {
    await new Promise((resolve) => setTimeout(resolve, 3600000)); // 1 hour
  }
}

logger.info('All agents created', { count: agentCount });
```

**Reference:** [FAQ - Performance](#performance-and-scalability)

---

### Q: How fast is balance tracking?

**A:** Balance tracking performance depends on caching and RPC endpoint:

**Performance Metrics:**

| Scenario                      | Response Time | Caching   |
| ----------------------------- | ------------- | --------- |
| **First Query** (Cold)        | 1-2 seconds   | No        |
| **Cached Query** (Hot)        | < 50ms        | Yes       |
| **Batch Query** (10 balances) | 2-3 seconds   | No        |
| **Polling Interval**          | 30 seconds    | Automatic |

**Optimization:**

```typescript
const balanceTracker = new AgentBalanceTracker({
  cacheEnabled: true,
  cacheTTL: 30000, // 30 seconds
  pollingInterval: 30000, // Poll every 30 seconds
});

// First call: Fetches from blockchain (slow)
const balance1 = await balanceTracker.getBalance('agent-001', 'evm', 'ETH');

// Subsequent calls: Returns from cache (fast)
const balance2 = await balanceTracker.getBalance('agent-001', 'evm', 'ETH');
// Returned in < 50ms
```

**Factors Affecting Performance:**

- RPC endpoint latency (Infura/Alchemy: ~100-200ms)
- Network congestion
- Number of tokens tracked per agent
- Cache configuration

**Reference:** [Troubleshooting - Slow Balance Queries](agent-wallet-troubleshooting.md#slow-balance-queries)

---

### Q: Can I batch create wallets?

**A:** Yes! Use parallel wallet creation, but respect rate limits:

**Efficient Batch Creation:**

```typescript
import { AgentWalletLifecycle } from '@crosstown/connector/wallet/agent-wallet-lifecycle';

async function batchCreateWallets(agentIds: string[]) {
  const lifecycle = new AgentWalletLifecycle();

  // Limit batch size to rate limit (100/hour)
  const batchSize = 100;

  for (let i = 0; i < agentIds.length; i += batchSize) {
    const batch = agentIds.slice(i, i + batchSize);

    logger.info('Creating wallet batch', {
      batchNumber: Math.floor(i / batchSize) + 1,
      batchSize: batch.length,
    });

    // Create all wallets in parallel (within rate limit)
    const wallets = await Promise.all(batch.map((id) => lifecycle.createAgentWallet(id)));

    logger.info('Batch complete', { walletsCreated: wallets.length });

    // Wait 1 hour before next batch (rate limit window)
    if (i + batchSize < agentIds.length) {
      logger.info('Waiting for rate limit window reset (1 hour)');
      await new Promise((resolve) => setTimeout(resolve, 3600000));
    }
  }
}

// Create 500 wallets (5 batches of 100)
const agentIds = Array.from({ length: 500 }, (_, i) => `agent-batch-${i}`);
await batchCreateWallets(agentIds);
```

**Alternative: Use `batchDeriveWallets()` (Future)**

```typescript
import { AgentWalletDerivation } from '@crosstown/connector/wallet/agent-wallet-derivation';

const derivation = new AgentWalletDerivation();

// Derive wallets in single operation (up to 1000 at once)
const wallets = await derivation.batchDeriveWallets(agentIds);

logger.info('Batch derivation complete', { count: wallets.length });
```

**Reference:** [API Reference - batchDeriveWallets()](../api/agent-wallet-api.md#batchderivewallets)

---

## Security

### Q: Are private keys ever exposed?

**A:** No. Private keys are **never** exposed in logs, telemetry, or API responses.

**Protection Mechanisms:**

1. **Pino Logger Serializers** (Automatic)

   ```typescript
   // Wallet objects automatically sanitized
   logger.info('Wallet created', { wallet }); // privateKey removed
   ```

2. **Telemetry Sanitization**

   ```typescript
   import { sanitizeWalletForTelemetry } from '@crosstown/connector/wallet/wallet-security';

   const sanitized = sanitizeWalletForTelemetry(wallet);
   // sanitized.privateKey = undefined
   ```

3. **API Response Filtering**
   ```typescript
   // API responses never include private keys
   const walletData = await lifecycle.getAgentWallet('agent-001');
   // Returns: { agentId, evmAddress, status }
   // Does NOT return: { privateKey, mnemonic, seed }
   ```

**Verification:**
Run security penetration tests to verify:

```bash
npm test -- --testPathPattern=wallet-security-penetration
```

**Reference:** [Security Best Practices - Key Protection](../security/agent-wallet-security.md#key-protection)

---

### Q: What authentication is required?

**A:** Sensitive wallet operations require one of three authentication methods:

**Authentication Methods:**

1. **Password** (MVP - Available Now)
   - Minimum 16 characters
   - PBKDF2 hash verification (100k iterations)
   - Timing-safe comparison

2. **2FA (TOTP)** (Epic 12)
   - 6-digit codes
   - 30-second time window
   - Google Authenticator, Authy compatible

3. **HSM** (Epic 12)
   - Hardware Security Module
   - AWS KMS, HashiCorp Vault support
   - Private keys never leave HSM

**Example:**

```typescript
import { AgentWalletAuthentication } from '@crosstown/connector/wallet/wallet-authentication';

const auth = new AgentWalletAuthentication();

// Password authentication
await auth.authenticate({
  method: 'password',
  password: 'strong-password-123456789',
});

// Now authorized to perform wallet operations
const wallet = await derivation.deriveAgentWallet('agent-001');
```

**Operations Requiring Authentication:**

- Wallet derivation
- Master seed access
- Backup creation
- Backup restoration

**Reference:** [Security Best Practices - Authentication Methods](../security/agent-wallet-security.md#authentication-methods)

---

### Q: How are spending limits enforced?

**A:** Security manager validates all transactions before signing, enforcing three spending limits:

**Spending Limits:**

1. **Max Transaction Size** (per transaction)
   - Default: 1000 USDC
   - Prevents single large unauthorized transfer

2. **Daily Limit** (rolling 24 hours)
   - Default: 5000 USDC
   - Prevents daily fund depletion

3. **Monthly Limit** (rolling 30 days)
   - Default: 50,000 USDC
   - Long-term spending cap

**Enforcement Flow:**

```typescript
// Transaction validation (automatic)
try {
  await channelManager.sendPayment(
    'agent-001',
    'channel-001',
    BigInt(150000000) // 150 USDC
  );
} catch (error) {
  if (error.message.includes('Spending limit exceeded')) {
    logger.warn('Transaction rejected by spending limits', {
      agentId: 'agent-001',
      amount: '150000000',
      reason: error.message,
    });
    // Handle limit violation
  }
}
```

**Custom Limits:**

```typescript
import { AgentWalletSecurity } from '@crosstown/connector/wallet/wallet-security';

const security = new AgentWalletSecurity();

// Configure custom limits for VIP agent
await security.setSpendingLimits('agent-vip-001', {
  maxTransactionSize: BigInt(5000000000), // 5000 USDC
  dailyLimit: BigInt(25000000000), // 25,000 USDC
  monthlyLimit: BigInt(250000000000), // 250,000 USDC
});
```

**Reference:** [Security Best Practices - Spending Limits](../security/agent-wallet-security.md#spending-limits)

---

## Payment Channels

### Q: What are payment channels?

**A:** Payment channels enable instant, low-cost micropayments between agents without on-chain transactions for every payment.

**How They Work:**

1. **Open Channel**: Lock funds on-chain
2. **Send Payments**: Update channel balance off-chain (instant)
3. **Close Channel**: Settle final balance on-chain

**Benefits:**

- ⚡ **Instant**: Sub-second payment confirmation
- 💰 **Low Cost**: No gas fees per payment
- 🔒 **Secure**: Cryptographic balance proofs
- 📈 **Scalable**: Thousands of payments per second

**Example:**

```typescript
import { AgentChannelManager } from '@crosstown/connector/wallet/agent-channel-manager';

const channelManager = new AgentChannelManager();

// 1. Open channel (on-chain, ~15 seconds)
const channelId = await channelManager.openChannel(
  'agent-001',
  'peer-agent-002',
  'evm',
  'USDC',
  BigInt(1000000000) // 1000 USDC
);

// 2. Send payments (off-chain, instant)
for (let i = 0; i < 100; i++) {
  await channelManager.sendPayment(
    'agent-001',
    channelId,
    BigInt(10000000) // 10 USDC per payment
  );
  // Each payment takes < 1 second, no gas fees
}

// 3. Close channel (on-chain, ~15 seconds)
await channelManager.closeChannel('agent-001', channelId);
```

**Reference:** [Integration Guide - Payment Channel Usage](agent-wallet-integration.md#payment-channel-usage)

---

### Q: How many channels can an agent have?

**A:** **No hard limit**, but **50 channels per agent recommended** for optimal performance.

**Channel Limits:**

| Factor                  | Limit             | Reason                     |
| ----------------------- | ----------------- | -------------------------- |
| **Technical Maximum**   | Unlimited         | No protocol restriction    |
| **Recommended**         | 50 channels/agent | Memory and performance     |
| **Open Simultaneously** | 10-20 channels    | Blockchain resource limits |

**Scaling Strategies:**

1. **Close Unused Channels**

   ```typescript
   // Get all channels
   const channels = await channelManager.getAgentChannels('agent-001');

   // Close inactive channels (no payments in 7 days)
   const sevenDaysAgo = Date.now() - 7 * 24 * 60 * 60 * 1000;

   for (const channel of channels) {
     if (channel.lastPaymentAt < sevenDaysAgo) {
       await channelManager.closeChannel('agent-001', channel.id);
       logger.info('Closed inactive channel', { channelId: channel.id });
     }
   }
   ```

2. **Reuse Channels**

   ```typescript
   // Check for existing channel before opening
   const existingChannel = channels.find(c => c.peerId === peerId);

   if (existingChannel) {
     // Reuse existing channel
     await channelManager.sendPayment('agent-001', existingChannel.id, amount);
   } else {
     // Open new channel
     const newChannelId = await channelManager.openChannel(...);
   }
   ```

**Reference:** [FAQ - Payment Channels](#payment-channels)

---

## Integration and Development

### Q: What programming languages are supported?

**A:** The agent wallet system is implemented in **TypeScript/Node.js**, with client libraries available for multiple languages:

**Supported Languages:**

1. **TypeScript** (Primary)
   - Full API access
   - Type safety
   - Direct library imports

2. **JavaScript** (Node.js)
   - Same API as TypeScript
   - No type checking
   - ES6+ syntax

3. **Python** (HTTP API Client)
   - REST API access
   - Type hints (Python 3.10+)
   - HTTP requests library

**Code Examples:**

- TypeScript: `examples/typescript/agent-wallet-integration.ts`
- JavaScript: `examples/javascript/agent-wallet-integration.js`
- Python: `examples/python/agent_wallet_integration.py`

**Future Language Support:**

- Go (Epic 12)
- Rust (Epic 12)
- Java (Epic 13)

**Reference:** [Multi-Language Code Examples](../../examples/)

---

### Q: Do I need to run a blockchain node?

**A:** No! Use commercial RPC providers - no node infrastructure required.

**Recommended RPC Providers:**

**EVM (Base L2):**

- **Infura**: https://infura.io
- **Alchemy**: https://alchemy.com
- **Public RPC**: https://mainnet.base.org (rate limited)

**Configuration:**

```bash
# .env
EVM_RPC_ENDPOINT=https://base-mainnet.infura.io/v3/YOUR-API-KEY
```

**Fallback Configuration:**

```typescript
const rpcEndpoints = [
  'https://base-mainnet.infura.io/v3/YOUR-KEY', // Primary
  'https://base.llamarpc.com', // Fallback 1
  'https://mainnet.base.org', // Fallback 2 (rate limited)
];
```

**Reference:** [Troubleshooting - RPC Endpoint Issues](agent-wallet-troubleshooting.md#rpc-endpoint-unreachable)

---

### Q: How do I test wallet integration?

**A:** Use test networks and mock data for safe integration testing:

**Testing Strategy:**

1. **Local Testing** (No Real Funds)

   ```typescript
   // Use mock wallet manager
   import { MockWalletLifecycle } from '@crosstown/connector/test/mocks';

   const mockLifecycle = new MockWalletLifecycle();
   const wallet = await mockLifecycle.createAgentWallet('test-agent');
   // Returns mock wallet, no blockchain interaction
   ```

2. **Testnet Testing** (Free Test Tokens)

   ```bash
   # Use Base Sepolia testnet
   export EVM_RPC_ENDPOINT=https://sepolia.base.org
   export EVM_NETWORK=base-sepolia
   ```

3. **Integration Tests**

   ```bash
   # Run integration test suite
   npm test -- --testPathPattern=integration

   # Run wallet-specific tests
   npm test -- --testPathPattern=wallet
   ```

**Test Coverage:**

- Unit tests: `packages/connector/src/wallet/*.test.ts`
- Integration tests: `packages/connector/test/integration/`
- Security tests: `wallet-security-penetration.test.ts`

**Reference:** [Integration Guide](agent-wallet-integration.md)

---

## Troubleshooting

### Q: Why is my wallet creation failing?

**A:** Common causes and solutions:

**1. Master Seed Not Found**

```
Error: master-seed not found in storage
```

**Solution:** Initialize seed manager with `generateMasterSeed()` or import existing seed.

**2. Rate Limit Exceeded**

```
Error: Funding rate limit exceeded - max 100 wallets/hour
```

**Solution:** Wait 1 hour or adjust rate limit configuration.

**3. Wallet Already Exists**

```
Error: Wallet already exists for agent: agent-001
```

**Solution:** Use `getAgentWallet()` to retrieve existing wallet instead of creating.

**4. Insufficient Permissions**

```
Error: EACCES: permission denied
```

**Solution:** Check file permissions on `data/wallet/` directory (should be 700).

**Reference:** [Troubleshooting Guide](agent-wallet-troubleshooting.md#wallet-creation-problems)

---

### Q: How do I debug balance issues?

**A:** Follow this debugging checklist:

**1. Verify Wallet Status**

```typescript
const wallet = await lifecycle.getAgentWallet(agentId);

if (wallet.status !== 'active') {
  logger.error('Wallet not active', { status: wallet.status });
}
```

**2. Check On-Chain Balance**

```typescript
import { ethers } from 'ethers';

const provider = new ethers.JsonRpcProvider(process.env.EVM_RPC_ENDPOINT);
const balance = await provider.getBalance(wallet.evmAddress);

logger.info('On-chain balance', {
  address: wallet.evmAddress,
  balance: balance.toString(),
});
```

**3. Verify RPC Endpoint**

```bash
# Test EVM RPC
curl -X POST $EVM_RPC_ENDPOINT \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}'
```

**4. Check Transaction History**

```typescript
const balances = await balanceTracker.getAllBalances(agentId);

balances.forEach((b) => {
  logger.info('Balance details', {
    chain: b.chain,
    token: b.token,
    balance: b.balance.toString(),
    lastUpdated: b.lastUpdated,
  });
});
```

**Reference:** [Troubleshooting Guide - Funding and Balance Issues](agent-wallet-troubleshooting.md#funding-and-balance-issues)

---

## Additional Resources

- **Integration Guide**: [Agent Wallet Integration](agent-wallet-integration.md)
- **API Reference**: [Agent Wallet API Documentation](../api/agent-wallet-api.md)
- **Security**: [Security Best Practices](../security/agent-wallet-security.md)
- **Troubleshooting**: [Troubleshooting Guide](agent-wallet-troubleshooting.md)
- **Production**: [Production Readiness Checklist](../operators/production-readiness-checklist.md)
- **Code Examples**: [Multi-Language Examples](../../examples/)

---

## Still Have Questions?

**Support Channels:**

- **GitHub Issues**: https://github.com/interledger/m2m/issues
- **Documentation**: https://docs.interledger.org/m2m
- **Community Forum**: https://forum.interledger.org
- **Security Issues**: security@interledger.org

**Before Asking:**

1. Check this FAQ
2. Review troubleshooting guide
3. Search existing GitHub issues
4. Read relevant documentation sections
