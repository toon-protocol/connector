#!/bin/sh
# Anvil initialization script - deploys contracts on startup

echo "🔧 Initializing Anvil with contract deployments..."

# Deploy contracts using forge script
forge script script/DeployLocal.s.sol:DeployLocalScript \
  --rpc-url http://localhost:8545 \
  --broadcast \
  --skip-simulation

echo "✅ Contracts deployed!"
echo "   AGENT Token:  0x5FbDB2315678afecb367f032d93F642f64180aa3"
echo "   TokenNetwork: 0xe7f1725e7734ce288f8367e1bb143e90bb3f0512"
