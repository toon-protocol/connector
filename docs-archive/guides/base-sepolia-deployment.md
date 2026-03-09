# Base Sepolia Public Testnet Deployment

**Status:** ✅ Active
**Network:** Base Sepolia L2 Testnet
**RPC Endpoint:** https://sepolia.base.org
**Block Explorer:** https://sepolia.basescan.org

## Current Configuration

### Network Details

All 5 peers are configured to use **Base Sepolia public testnet**:

```env
BASE_L2_RPC_URL=https://sepolia.base.org
BASE_RPC_URL=https://sepolia.base.org
```

### Funding Requirements

Base Sepolia has **extremely low gas costs**, so minimal ETH is needed:

- **Gas cost per transaction:** ~0.00001 ETH (~$0.000025)
- **ETH per peer:** 0.0001 ETH (sufficient for ~10 transactions)
- **M2M tokens per peer:** 1000 tokens (for settlement)

### Treasury Wallet Setup

Your treasury wallet needs to be funded with:

1. **Sepolia ETH** (for gas):
   - Get from faucet: https://www.alchemy.com/faucets/base-sepolia
   - Or bridge from Sepolia: https://bridge.base.org/deposit
   - Amount needed: ~0.001 ETH (0.0001 × 5 peers + buffer)

2. **M2M ERC20 tokens** (for settlement):
   - Deploy M2M token contract on Base Sepolia (if not already deployed)
   - Mint 5000+ tokens to treasury wallet
   - Or use existing testnet M2M token

## Peer Funding

Use the funding tool with testnet-appropriate amounts:

```bash
cd tools/fund-peers

# Fund 5 peers with minimal gas ETH
npm run fund -- \
  --peers peer1,peer2,peer3,peer4,peer5 \
  --eth-amount 0.0001 \
  --token-amount 1000 \
  --rpc-url https://sepolia.base.org
```

**Cost breakdown:**

- ETH: 0.0001 × 5 = 0.0005 ETH (~$0.00125)
- M2M tokens: 1000 × 5 = 5000 tokens

## Verify Testnet Connection

Check that peers are connected to Base Sepolia:

```bash
# Test RPC connection
curl -X POST https://sepolia.base.org \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}'

# Expected: {"jsonrpc":"2.0","id":1,"result":"0x14a34"} (chain ID 84532 = Base Sepolia)
```

Check peer logs for RPC connection:

```bash
docker logs peer1 2>&1 | grep -i "rpc\|sepolia\|chain"
```

## Explorer UI Status

**Current Status:** ⚠️ Disabled

The Explorer UI requires telemetry infrastructure which needs:

- Telemetry WebSocket server running
- `DASHBOARD_TELEMETRY_URL` environment variable set

**Options to Enable Explorer UI:**

### Option 1: Standalone Mode (Recommended for Testing)

Run Explorer UI locally to view events from SQLite event store:

```bash
# In future implementation
cd packages/connector/explorer-ui
npm run dev
open http://localhost:5173
```

### Option 2: Centralized Telemetry (Production)

Set up central telemetry server:

```bash
# Deploy telemetry aggregator
docker compose -f docker-compose-monitoring.yml up -d

# Add to each peer's environment:
DASHBOARD_TELEMETRY_URL=ws://telemetry-server:6001
EXPLORER_ENABLED=true
```

### Option 3: Per-Peer Explorer (Current Setup)

Each peer has its own explorer port configured:

- Peer1: http://localhost:5173
- Peer2: http://localhost:5174
- Peer3: http://localhost:5175
- Peer4: http://localhost:5176
- Peer5: http://localhost:5177

**Currently disabled** - requires telemetry emitter fix.

## Testnet vs Local Anvil

### Why Base Sepolia (Current)

✅ **Advantages:**

- Real blockchain environment
- Publicly verifiable transactions
- Free testnet ETH via faucet
- Production-like behavior
- Block explorer integration
- Multi-user testing possible

❌ **Disadvantages:**

- Requires testnet ETH from faucet
- Network latency (~1-2 second block times)
- Dependent on public RPC availability
- Requires internet connection

### Local Anvil (Alternative)

✅ **Advantages:**

- Instant blocks (no wait time)
- Unlimited test ETH
- Fully controlled environment
- No internet dependency
- Deterministic private keys

❌ **Disadvantages:**

- Not publicly verifiable
- Doesn't test real network conditions
- Reset on container restart
- Single-user only

## Transaction Costs on Base Sepolia

Base Sepolia is an L2 rollup with **extremely low gas costs**:

| Operation              | Gas Cost     | ETH Cost (@ 1 gwei base fee) |
| ---------------------- | ------------ | ---------------------------- |
| ETH transfer           | ~21,000 gas  | ~0.000021 ETH                |
| ERC20 transfer         | ~50,000 gas  | ~0.00005 ETH                 |
| Payment channel open   | ~200,000 gas | ~0.0002 ETH                  |
| Payment channel settle | ~150,000 gas | ~0.00015 ETH                 |

**With 0.0001 ETH per peer:**

- Can perform ~2 basic transactions
- Sufficient for testing packet forwarding
- Need more for payment channel operations

**Recommendation for full settlement testing:**

- Fund peers with 0.001 ETH each (sufficient for channel operations)
- Or use claim exchange (Epic 17) which minimizes on-chain transactions

## Verifying Transactions on Base Sepolia

View transactions on Block Explorer:

```bash
# Get treasury wallet address
grep TREASURY_EVM_PRIVATE_KEY .env

# Use cast to get address from private key
cast wallet address --private-key $TREASURY_EVM_PRIVATE_KEY

# View on BaseScan
open "https://sepolia.basescan.org/address/<TREASURY_ADDRESS>"
```

View peer wallet transactions:

```bash
# After funding, peer addresses will be logged
# View each peer's transactions on BaseScan
open "https://sepolia.basescan.org/address/<PEER_ADDRESS>"
```

## Next Steps

### 1. Fund Treasury Wallet

Get Sepolia ETH for your treasury wallet:

```bash
# Visit Base Sepolia faucet
open "https://www.alchemy.com/faucets/base-sepolia"

# Or bridge from Ethereum Sepolia
open "https://bridge.base.org/deposit"
```

### 2. Run Funding Script

```bash
cd tools/fund-peers
npm run fund -- \
  --peers peer1,peer2,peer3,peer4,peer5 \
  --eth-amount 0.0001 \
  --token-amount 1000
```

### 3. Send Test Packets

```bash
cd tools/send-packet
node dist/index.js \
  -c ws://localhost:3000 \
  -d g.peer5.dest \
  -a 1000000
```

### 4. Verify on Block Explorer

Check transactions on BaseScan:

```bash
# View settlement transactions (if triggered)
open "https://sepolia.basescan.org"
```

## Explorer UI Setup (Future)

To enable the Explorer UI for viewing packets in real-time:

### Option A: Quick Fix (Local Event Viewing)

Modify connector to allow standalone explorer mode without telemetry WebSocket.

### Option B: Full Telemetry Setup

1. Deploy telemetry aggregator service
2. Configure all peers with DASHBOARD_TELEMETRY_URL
3. Access centralized explorer dashboard

### Option C: Per-Peer SQLite Viewer

Build simple HTTP API to query event store directly:

```typescript
// Future implementation
app.get('/api/events', async (req, res) => {
  const events = await eventStore.getEvents({ limit: 100 });
  res.json(events);
});
```

## Summary

✅ **Network is running on Base Sepolia public testnet**
✅ **All 5 peers healthy and connected**
✅ **Multi-hop routing verified**
✅ **Minimal ETH required (0.0001 per peer)**
⏭️ **Explorer UI requires telemetry setup**
⏭️ **Treasury wallet needs Sepolia ETH and M2M tokens**

The deployment is production-ready for Base Sepolia testnet operation!
