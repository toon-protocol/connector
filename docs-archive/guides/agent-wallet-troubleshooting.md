# Agent Wallet Troubleshooting Guide

Common issues and solutions for agent wallet integration and operations.

## Table of Contents

1. [Master Seed Issues](#master-seed-issues)
2. [Wallet Creation Problems](#wallet-creation-problems)
3. [Funding and Balance Issues](#funding-and-balance-issues)
4. [Payment Channel Errors](#payment-channel-errors)
5. [Rate Limit Violations](#rate-limit-violations)
6. [Backup and Recovery Problems](#backup-and-recovery-problems)
7. [Network and Connectivity Issues](#network-and-connectivity-issues)
8. [Performance Issues](#performance-issues)

---

## Master Seed Issues

### Issue: Master Seed Not Found

**Symptom:**

```
Error: master-seed not found in storage
```

**Cause:**
Seed manager not initialized, or seed file missing from storage.

**Solution:**

**Option 1: Initialize New Seed (New Deployment)**

```typescript
import { AgentSeedManager } from '@crosstown/connector/wallet/agent-seed-manager';

const seedManager = new AgentSeedManager();

// Generate new master seed
await seedManager.generateMasterSeed('strong-password-min-16-chars');

logger.info('Master seed generated successfully');
```

**Option 2: Import Existing Seed (Recovery)**

```typescript
// If you have backed up mnemonic
const mnemonic = '...24 word mnemonic phrase...';

await seedManager.importMasterSeed(mnemonic, 'strong-password-min-16-chars');

logger.info('Master seed imported successfully');
```

**Prevention:**

- Always backup master seed after generation
- Store encrypted backups in multiple locations
- Test backup restore regularly

**Reference:** `packages/connector/src/wallet/agent-seed-manager.ts`

---

### Issue: Invalid Master Seed Password

**Symptom:**

```
Error: Failed to decrypt master seed: invalid password
```

**Cause:**
Incorrect password provided for seed decryption.

**Solution:**

1. **Verify Password:**
   - Check for typos
   - Verify caps lock is off
   - Try password from secure password manager

2. **Reset Password (If Backup Available):**

   ```typescript
   // Restore from backup with correct password
   const backup = await loadBackup('backup-file.enc');
   await backupManager.restoreFromBackup(backup, correctPassword);
   ```

3. **Last Resort (If No Backup):**
   - Master seed is permanently lost
   - All agent wallets are irrecoverable
   - Must generate new master seed
   - Create new wallets for all agents

**Prevention:**

- Store password in enterprise password manager
- Document password recovery procedure
- Test password regularly

---

### Issue: Master Seed File Corrupted

**Symptom:**

```
Error: Failed to decrypt master seed: malformed data
```

**Cause:**
Seed file corrupted due to disk error or interrupted write.

**Solution:**

1. **Check File Permissions:**

   ```bash
   ls -la data/wallet/master-seed.enc
   # Should show: -rw------- (600 permissions)
   ```

2. **Verify File Integrity:**

   ```bash
   # Check file size (should be > 100 bytes)
   wc -c data/wallet/master-seed.enc
   ```

3. **Restore from Backup:**

   ```typescript
   import { WalletBackupManager } from '@crosstown/connector/wallet/wallet-backup-manager';

   const backupManager = new WalletBackupManager();
   const backup = await loadBackup('backup-weekly.enc');

   await backupManager.restoreFromBackup(backup, password);
   ```

**Prevention:**

- Use reliable storage (SSD with ECC)
- Enable filesystem journaling
- Maintain multiple backup copies

---

## Wallet Creation Problems

### Issue: Wallet Already Exists for Agent

**Symptom:**

```
Error: Wallet already exists for agent: agent-001
```

**Cause:**
Agent ID already has a derived wallet.

**Solution:**

**Use Existing Wallet:**

```typescript
import { AgentWalletLifecycle } from '@crosstown/connector/wallet/agent-wallet-lifecycle';

const lifecycle = new AgentWalletLifecycle();

// Retrieve existing wallet instead of creating
const wallet = await lifecycle.getAgentWallet('agent-001');

if (wallet) {
  logger.info('Using existing wallet', {
    agentId: wallet.agentId,
    evmAddress: wallet.evmAddress,
  });
} else {
  // Wallet doesn't exist, safe to create
  const newWallet = await lifecycle.createAgentWallet('agent-001');
}
```

**If You Need New Wallet:**

1. Archive old wallet:

   ```typescript
   await lifecycle.archiveWallet('agent-001');
   ```

2. Create new wallet with different agent ID:
   ```typescript
   const newWallet = await lifecycle.createAgentWallet('agent-001-v2');
   ```

**Prevention:**

- Check if wallet exists before creating
- Use unique agent IDs
- Document agent ID naming convention

---

### Issue: Wallet Derivation Index Collision

**Symptom:**

```
Error: Derivation index collision at index 42
```

**Cause:**
Internal database inconsistency - derivation index already used by another agent.

**Solution:**

1. **Check Agent Mapping:**

   ```typescript
   import { AgentWalletDerivation } from '@crosstown/connector/wallet/agent-wallet-derivation';

   const derivation = new AgentWalletDerivation();

   // Find which agent uses this index
   const conflictingWallet = await derivation.getWalletByIndex(42);
   logger.info('Index 42 used by', { agentId: conflictingWallet.agentId });
   ```

2. **Database Repair (Advanced):**

   ```bash
   # Backup database first
   cp data/wallet/agent-wallets.db data/wallet/agent-wallets.db.bak

   # Verify database integrity
   sqlite3 data/wallet/agent-wallets.db "PRAGMA integrity_check;"
   ```

3. **Contact Support:**
   - This indicates a bug - report to development team
   - Provide agent ID and derivation index

**Prevention:**

- Regular database integrity checks
- Atomic transactions for wallet creation
- Database backups before major operations

---

## Funding and Balance Issues

### Issue: Insufficient Funds for Gas

**Symptom:**

```
Error: Transaction failed: insufficient funds for gas
```

**Cause:**
EVM wallet needs native ETH for gas fees, but balance is zero or too low.

**Solution:**

1. **Check ETH Balance:**

   ```typescript
   import { AgentBalanceTracker } from '@crosstown/connector/wallet/agent-balance-tracker';

   const balanceTracker = new AgentBalanceTracker();
   const ethBalance = await balanceTracker.getBalance('agent-001', 'evm', 'ETH');

   logger.info('ETH balance', { balance: ethBalance.toString() });
   // Need at least 0.001 ETH (~$3 at $3000/ETH) for gas
   ```

2. **Fund Wallet with ETH:**

   ```typescript
   import { AgentWalletFunder } from '@crosstown/connector/wallet/agent-wallet-funder';

   const funder = new AgentWalletFunder();

   await funder.fundAgentWallet('agent-001', {
     chain: 'evm',
     token: 'ETH',
     amount: BigInt('100000000000000000'), // 0.1 ETH
   });

   logger.info('Agent wallet funded with ETH');
   ```

3. **Wait for Confirmation:**

   ```typescript
   // Poll for balance update
   let balance = ethBalance;
   while (balance === ethBalance) {
     await new Promise((resolve) => setTimeout(resolve, 5000));
     balance = await balanceTracker.getBalance('agent-001', 'evm', 'ETH');
   }

   logger.info('Funding confirmed', { newBalance: balance.toString() });
   ```

**Prevention:**

- Automatic funding includes ETH for gas (0.1 ETH)
- Monitor ETH balance (alert if < 0.01 ETH)
- Implement automatic gas top-up

**Reference:** `packages/connector/src/wallet/agent-wallet-funder.ts`

---

### Issue: Balance Mismatch After Backup Restore

**Symptom:**

```
Warning: Restored balance (1000 USDC) != on-chain balance (850 USDC)
```

**Cause:**
Transactions occurred after backup timestamp - backup is stale.

**Solution:**

**Automatic Reconciliation (Built-in):**

```typescript
import { WalletBackupManager } from '@crosstown/connector/wallet/wallet-backup-manager';

const backupManager = new WalletBackupManager();

// Restore triggers automatic balance reconciliation
await backupManager.restoreFromBackup(backup, password);

logger.info('Backup restored - balances reconciled with on-chain data');
```

**Manual Reconciliation (If Needed):**

```typescript
import { AgentBalanceTracker } from '@crosstown/connector/wallet/agent-balance-tracker';

const balanceTracker = new AgentBalanceTracker();

// Force balance refresh from blockchain
await balanceTracker.refreshBalance('agent-001', 'evm', 'USDC');

const currentBalance = await balanceTracker.getBalance('agent-001', 'evm', 'USDC');
logger.info('Balance reconciled', { balance: currentBalance.toString() });
```

**Understanding the Mismatch:**

- Backup captures balance at snapshot time
- Transactions after backup change on-chain balance
- Restore process reconciles with current on-chain state
- This is expected behavior, not an error

**Prevention:**

- Take backups immediately after transactions
- Document time of last backup
- Test restore in isolated environment first

---

## Payment Channel Errors

### Issue: Channel Already Exists

**Symptom:**

```
Error: Channel already exists between agent-001 and peer-agent-002
```

**Cause:**
Payment channel already open with the specified peer.

**Solution:**

**Option 1: Use Existing Channel**

```typescript
import { AgentChannelManager } from '@crosstown/connector/wallet/agent-channel-manager';

const channelManager = new AgentChannelManager();

// Get existing channels
const channels = await channelManager.getAgentChannels('agent-001');

// Find channel with peer
const existingChannel = channels.find((c) => c.peerId === 'peer-agent-002');

if (existingChannel) {
  logger.info('Using existing channel', { channelId: existingChannel.id });

  // Send payment through existing channel
  await channelManager.sendPayment('agent-001', existingChannel.id, BigInt(10000000));
}
```

**Option 2: Close and Reopen**

```typescript
// Close existing channel
await channelManager.closeChannel('agent-001', existingChannel.id);

logger.info('Old channel closed');

// Wait for settlement
await new Promise((resolve) => setTimeout(resolve, 20000)); // 20 seconds

// Open new channel
const newChannelId = await channelManager.openChannel(
  'agent-001',
  'peer-agent-002',
  'evm',
  'USDC',
  BigInt(2000000000) // 2000 USDC
);

logger.info('New channel opened', { channelId: newChannelId });
```

**Prevention:**

- Check for existing channels before opening
- Maintain channel inventory
- Close unused channels promptly

---

### Issue: Insufficient Channel Balance

**Symptom:**

```
Error: Payment exceeds remaining channel balance
```

**Cause:**
Payment amount greater than channel's remaining balance.

**Solution:**

1. **Check Channel Balance:**

   ```typescript
   const channels = await channelManager.getAgentChannels('agent-001');
   const channel = channels.find((c) => c.id === 'channel-evm-001');

   logger.info('Channel balance', {
     channelId: channel.id,
     balance: channel.balance.toString(),
     initialAmount: channel.initialAmount.toString(),
   });
   ```

2. **Option A: Send Smaller Payment**

   ```typescript
   // Send payment within available balance
   const paymentAmount = channel.balance / BigInt(2); // Half of balance

   await channelManager.sendPayment('agent-001', channel.id, paymentAmount);
   ```

3. **Option B: Close and Open Larger Channel**

   ```typescript
   // Close depleted channel
   await channelManager.closeChannel('agent-001', channel.id);

   // Open new channel with more funds
   const newChannelId = await channelManager.openChannel(
     'agent-001',
     channel.peerId,
     channel.chain,
     channel.token,
     BigInt(5000000000) // 5000 USDC
   );
   ```

**Prevention:**

- Monitor channel balances
- Open channels with sufficient capacity
- Implement automatic channel rebalancing

---

### Issue: Channel Opening Timeout

**Symptom:**

```
Error: Channel opening timed out after 120 seconds
```

**Cause:**
Blockchain congestion or RPC endpoint issues delaying confirmation.

**Solution:**

1. **Check Blockchain Status:**

   ```bash
   # EVM (Base L2) - check block explorer
   curl https://base.blockscout.com/api/v2/stats
   ```

2. **Retry with Higher Gas (EVM only):**

   ```typescript
   // Retry with higher gas price
   const channelId = await channelManager.openChannel(
     'agent-001',
     'peer-agent-002',
     'evm',
     'USDC',
     BigInt(1000000000),
     { gasPrice: BigInt('50000000000') } // 50 gwei
   );
   ```

3. **Check RPC Endpoint:**

   ```typescript
   import { ethers } from 'ethers';

   const provider = new ethers.JsonRpcProvider(process.env.EVM_RPC_ENDPOINT);

   try {
     const blockNumber = await provider.getBlockNumber();
     logger.info('RPC endpoint healthy', { blockNumber });
   } catch (error) {
     logger.error('RPC endpoint unreachable', { error: error.message });
   }
   ```

**Prevention:**

- Use reliable RPC providers (Infura, Alchemy)
- Implement RPC endpoint failover
- Set appropriate timeout values (300s for congestion)
- Monitor blockchain congestion metrics

---

## Rate Limit Violations

### Issue: Funding Rate Limit Exceeded

**Symptom:**

```
Error: Funding rate limit exceeded - max 100 wallets/hour
```

**Cause:**
Exceeded maximum wallet creations per hour (default: 100).

**Solution:**

1. **Wait for Rate Limit Window to Reset:**

   ```typescript
   // Rate limit is sliding window (1 hour)
   const waitTime = 60 * 60 * 1000; // 1 hour in milliseconds

   logger.info('Rate limit exceeded - waiting', { waitTimeMs: waitTime });

   await new Promise((resolve) => setTimeout(resolve, waitTime));

   // Retry wallet creation
   const wallet = await lifecycle.createAgentWallet('agent-new');
   ```

2. **Batch Wallet Creation (If Needed):**

   ```typescript
   // Create wallets in smaller batches
   const agentIds = ['agent-100', 'agent-101', ...]; // 200 agents

   // Batch 1: First 100 agents
   const batch1 = agentIds.slice(0, 100);
   for (const id of batch1) {
     await lifecycle.createAgentWallet(id);
   }

   logger.info('Batch 1 complete - waiting 1 hour');
   await new Promise(resolve => setTimeout(resolve, 60 * 60 * 1000));

   // Batch 2: Next 100 agents
   const batch2 = agentIds.slice(100, 200);
   for (const id of batch2) {
     await lifecycle.createAgentWallet(id);
   }
   ```

3. **Adjust Rate Limit (Production Only):**
   ```bash
   # Environment variable
   export WALLET_CREATION_RATE_LIMIT=500
   export WALLET_CREATION_RATE_WINDOW=3600  # seconds
   ```

**Prevention:**

- Plan bulk wallet creation in advance
- Stagger wallet creation over multiple hours
- Request rate limit increase for production

**Reference:** `packages/connector/src/wallet/rate-limiter.ts`

---

### Issue: Payment Rate Limit Exceeded

**Symptom:**

```
Error: Payment rate limit exceeded - max 1000 payments/minute
```

**Cause:**
Too many payments sent through channels in short time period.

**Solution:**

1. **Implement Payment Queuing:**

   ```typescript
   class PaymentQueue {
     private queue: Payment[] = [];
     private processing = false;

     async add(payment: Payment) {
       this.queue.push(payment);
       if (!this.processing) {
         await this.process();
       }
     }

     private async process() {
       this.processing = true;

       while (this.queue.length > 0) {
         const payment = this.queue.shift();

         try {
           await channelManager.sendPayment(payment.agentId, payment.channelId, payment.amount);

           // Rate limit: 1000/minute = ~60ms between payments
           await new Promise((resolve) => setTimeout(resolve, 60));
         } catch (error) {
           logger.error('Payment failed', { error: error.message });
           // Re-queue or handle error
         }
       }

       this.processing = false;
     }
   }
   ```

2. **Batch Payments:**

   ```typescript
   // Instead of many small payments, send fewer larger payments
   const smallPayments = [
     BigInt(1000000), // 1 USDC
     BigInt(2000000), // 2 USDC
     BigInt(3000000), // 3 USDC
   ];

   const totalAmount = smallPayments.reduce((sum, amt) => sum + amt, BigInt(0));

   // Send one payment for total
   await channelManager.sendPayment('agent-001', channelId, totalAmount);
   ```

**Prevention:**

- Design for sustainable payment rates
- Use payment queuing system
- Monitor payment velocity

---

## Backup and Recovery Problems

### Issue: Invalid Backup Password

**Symptom:**

```
Error: Invalid password - cannot decrypt backup
```

**Cause:**
Wrong password provided for backup decryption.

**Solution:**

1. **Try Alternative Passwords:**
   - Check password manager for backup password
   - Try passwords from different environments (dev, staging, prod)
   - Verify password doesn't have hidden characters

2. **Check Backup Metadata:**

   ```typescript
   import { readFile } from 'fs/promises';

   const backupJson = await readFile('backup-2026-01-21.enc', 'utf-8');
   const backup = JSON.parse(backupJson);

   logger.info('Backup metadata', {
     id: backup.id,
     createdAt: backup.createdAt,
     walletCount: backup.wallets.length,
   });
   ```

3. **Use Different Backup:**
   - Try previous backup with known password
   - Test restore in isolated environment first

**Prevention:**

- Store backup passwords in enterprise password manager
- Test backup restore with correct password quarterly
- Document password recovery procedure

**Reference:** `packages/connector/src/wallet/wallet-backup-manager.ts`

---

### Issue: Corrupt Backup Data

**Symptom:**

```
Error: Corrupt backup data - decryption failed
```

**Cause:**
Backup file corrupted during storage or transfer.

**Solution:**

1. **Verify Backup Integrity:**

   ```bash
   # Check file size
   ls -lh backup-2026-01-21.enc
   # Should be > 1KB

   # Verify JSON structure
   jq '.' backup-2026-01-21.enc > /dev/null
   echo $?
   # Should output 0 (success)
   ```

2. **Try Alternative Backup:**

   ```typescript
   // List all available backups
   const backups = ['backup-daily.enc', 'backup-weekly.enc', 'backup-monthly.enc'];

   for (const backupFile of backups) {
     try {
       const backup = await loadBackup(backupFile);
       await backupManager.restoreFromBackup(backup, password);

       logger.info('Backup restored successfully', { backupFile });
       break; // Success - stop trying
     } catch (error) {
       logger.warn('Backup failed', { backupFile, error: error.message });
       // Try next backup
     }
   }
   ```

3. **Restore from Off-site Backup:**
   - Retrieve backup from cloud storage (S3, GCS)
   - Use offline backup if network backups corrupted

**Prevention:**

- Store backups in multiple locations
- Verify backup integrity after creation
- Test restore procedure regularly

---

## Network and Connectivity Issues

### Issue: RPC Endpoint Unreachable

**Symptom:**

```
Error: Failed to connect to EVM RPC endpoint
```

**Cause:**
RPC provider down or network connectivity issues.

**Solution:**

1. **Test RPC Connectivity:**

   ```bash
   # EVM (Base L2)
   curl -X POST https://mainnet.base.org \
     -H "Content-Type: application/json" \
     -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}'
   ```

2. **Configure Fallback RPC:**

   ```typescript
   import { ethers } from 'ethers';

   const rpcEndpoints = [
     'https://mainnet.base.org',
     'https://base.llamarpc.com',
     'https://base-rpc.publicnode.com',
   ];

   let provider: ethers.JsonRpcProvider | null = null;

   for (const endpoint of rpcEndpoints) {
     try {
       const testProvider = new ethers.JsonRpcProvider(endpoint);
       await testProvider.getBlockNumber(); // Test connection

       provider = testProvider;
       logger.info('Connected to RPC', { endpoint });
       break;
     } catch (error) {
       logger.warn('RPC endpoint failed', { endpoint, error: error.message });
     }
   }

   if (!provider) {
     throw new Error('All RPC endpoints unreachable');
   }
   ```

3. **Use Commercial RPC Provider:**

   ```bash
   # Infura
   export EVM_RPC_ENDPOINT="https://base-mainnet.infura.io/v3/YOUR-API-KEY"

   # Alchemy
   export EVM_RPC_ENDPOINT="https://base-mainnet.g.alchemy.com/v2/YOUR-API-KEY"
   ```

**Prevention:**

- Use commercial RPC providers (Infura, Alchemy)
- Configure multiple RPC endpoints
- Implement automatic failover
- Monitor RPC endpoint health

---

### Issue: Blockchain Transaction Stuck

**Symptom:**

```
Warning: Transaction pending for 10 minutes without confirmation
```

**Cause:**
Low gas price (EVM) or network congestion.

**Solution:**

**For EVM (Base L2):**

```typescript
import { ethers } from 'ethers';

// Check transaction status
const provider = new ethers.JsonRpcProvider(process.env.EVM_RPC_ENDPOINT);
const tx = await provider.getTransaction(txHash);

if (tx && !tx.blockNumber) {
  logger.warn('Transaction still pending', { txHash });

  // Speed up transaction with higher gas
  const speedUpTx = await wallet.sendTransaction({
    ...tx,
    gasPrice: (tx.gasPrice! * BigInt(150)) / BigInt(100), // 1.5x gas price
    nonce: tx.nonce,
  });

  logger.info('Speed-up transaction sent', { newTxHash: speedUpTx.hash });
}
```

**Prevention:**

- Use appropriate gas prices (check current network gas)
- Set reasonable transaction timeouts
- Implement transaction monitoring

---

## Performance Issues

### Issue: Slow Balance Queries

**Symptom:**
Balance queries taking > 5 seconds.

**Cause:**
RPC endpoint latency or balance not cached.

**Solution:**

1. **Enable Balance Caching:**

   ```typescript
   import { AgentBalanceTracker } from '@crosstown/connector/wallet/agent-balance-tracker';

   const balanceTracker = new AgentBalanceTracker({
     cacheEnabled: true,
     cacheTTL: 30000, // 30 seconds
     pollingInterval: 30000, // Poll every 30 seconds
   });

   // First call: Slow (fetches from blockchain)
   const balance1 = await balanceTracker.getBalance('agent-001', 'evm', 'ETH');

   // Subsequent calls: Fast (returns from cache)
   const balance2 = await balanceTracker.getBalance('agent-001', 'evm', 'ETH');
   ```

2. **Use Faster RPC Endpoint:**
   - Switch to commercial provider (Infura, Alchemy)
   - Use regional RPC endpoint closer to your server

3. **Batch Balance Queries:**

   ```typescript
   // Instead of individual queries
   // const eth = await balanceTracker.getBalance('agent-001', 'evm', 'ETH');
   // const usdc = await balanceTracker.getBalance('agent-001', 'evm', 'USDC');

   // Use getAllBalances (single RPC call)
   const balances = await balanceTracker.getAllBalances('agent-001');
   ```

**Performance Benchmarks:**

- **Target**: Balance query < 1 second
- **Cached**: Balance query < 50ms
- **Batch**: 10 balances < 2 seconds

---

### Issue: High Memory Usage

**Symptom:**
Node.js process using > 2GB RAM.

**Cause:**
Too many cached balances or wallet objects in memory.

**Solution:**

1. **Check Memory Usage:**

   ```bash
   # Monitor Node.js heap
   node --max-old-space-size=4096 server.js --expose-gc

   # In separate terminal
   kill -USR2 $(pgrep -f server.js)
   # Check heap snapshot
   ```

2. **Configure Cache Limits:**

   ```typescript
   const balanceTracker = new AgentBalanceTracker({
     maxCachedAgents: 1000, // Limit cached agents
     cacheTTL: 30000, // Expire cache after 30s
     evictionPolicy: 'lru', // Least Recently Used eviction
   });
   ```

3. **Implement Garbage Collection:**
   ```typescript
   // Periodic GC (if --expose-gc flag set)
   setInterval(() => {
     if (global.gc) {
       global.gc();
       logger.info('Manual GC triggered');
     }
   }, 60000); // Every 1 minute
   ```

**Prevention:**

- Set appropriate cache size limits
- Use LRU eviction policy
- Monitor memory usage with metrics

---

## Getting Additional Help

### Log Collection for Support

When reporting issues, collect these logs:

```bash
# Connector logs (last 1000 lines)
tail -n 1000 logs/connector.log > issue-logs.txt

# Audit logs for agent
sqlite3 data/wallet/audit-log.db \
  "SELECT * FROM wallet_audit_log WHERE agentId='agent-001' ORDER BY timestamp DESC LIMIT 100;" \
  > audit-logs.txt

# System info
uname -a > system-info.txt
node --version >> system-info.txt
npm --version >> system-info.txt
```

### Support Resources

- **GitHub Issues**: https://github.com/interledger/m2m/issues
- **Documentation**: https://docs.interledger.org/m2m
- **Community Forum**: https://forum.interledger.org
- **Security Issues**: security@interledger.org (PGP key available)

### Before Contacting Support

1. Check this troubleshooting guide
2. Search existing GitHub issues
3. Review relevant documentation
4. Collect logs and error messages
5. Note exact steps to reproduce issue

---

## Related Documentation

- **Integration Guide**: [Agent Wallet Integration](agent-wallet-integration.md)
- **API Reference**: [Agent Wallet API](../api/agent-wallet-api.md)
- **Security**: [Security Best Practices](../security/agent-wallet-security.md)
- **FAQ**: [Frequently Asked Questions](agent-wallet-faq.md)
- **Production**: [Production Readiness Checklist](../operators/production-readiness-checklist.md)
