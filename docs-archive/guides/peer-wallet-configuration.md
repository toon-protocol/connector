# Peer Wallet vs Treasury Wallet

## The Problem

**Current (Wrong):**

- All peers use `TREASURY_EVM_PRIVATE_KEY` for transactions
- Treasury wallet: 0x4955808C589EDA5A5467871d6bB24D5613aC8304
- Peer wallets (0x6AFbC..., 0x62437..., etc.) are funded but NOT used

**Correct:**

- Each peer should use its OWN private key
- Treasury is only for initial funding
- Peer wallets should sign their own transactions

## Solution

### For Test Script

Since peer wallets were auto-generated and we didn't save the private keys, we have two options:

**Option 1: Use Treasury for All Peers (Quick Fix)**

- Fund the treasury wallet address on each peer container
- Not realistic but works for testing

**Option 2: Generate and Configure Individual Keys (Proper)**

- Generate 5 private keys
- Configure each peer with its own key
- Fund each peer's wallet
- Realistic production setup

### For Production

Each peer MUST have:

- Unique private key (NODE_PRIVATE_KEY)
- Funded wallet address
- Secure key management (HSM/KMS)

The treasury is ONLY used for:

- Initial funding of peer wallets
- Not for peer operations

## Configuration Needed

```yaml
# docker-compose-5-peer-multihop.yml
peer1:
  environment:
    NODE_PRIVATE_KEY: ${PEER1_PRIVATE_KEY} # NOT TREASURY_EVM_PRIVATE_KEY

peer2:
  environment:
    NODE_PRIVATE_KEY: ${PEER2_PRIVATE_KEY} # Each peer has own key
```
