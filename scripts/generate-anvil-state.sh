#!/bin/bash
# Generate Anvil state dump with pre-deployed contracts
#
# This creates a state file that Anvil can load instantly on startup,
# avoiding the slow forge compile + deploy step.
#
# Usage: ./scripts/generate-anvil-state.sh
#
# The generated state file is used by:
#   - docker-compose-base-e2e-test.yml
#   - docker-compose-evm-test.yml

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
STATE_FILE="$REPO_ROOT/packages/contracts/anvil-state.json"

echo "Generating Anvil state with pre-deployed contracts..."
echo ""

# Remove old state file
rm -f "$STATE_FILE"

# Run in a single Docker container:
# 1. Start Anvil with --state (dumps on exit)
# 2. Deploy contracts via forge script
# 3. Kill Anvil with SIGINT to trigger state dump
docker run --rm \
  --entrypoint sh \
  -v "$REPO_ROOT/packages/contracts:/contracts" \
  -w /contracts \
  ghcr.io/foundry-rs/foundry:latest \
  -c '
    export FOUNDRY_DISABLE_NIGHTLY_WARNING=1
    STATE_FILE=/contracts/anvil-state.json

    # Start Anvil with state persistence (dumps state on exit)
    anvil --host 0.0.0.0 --port 8545 --chain-id 31337 \
      --accounts 10 --balance 10000 \
      --state "$STATE_FILE" > /dev/null 2>&1 &

    echo "Waiting for Anvil..."
    until cast client --rpc-url http://localhost:8545 2>/dev/null | grep -q "anvil"; do
      sleep 0.5
    done
    echo "Anvil ready"

    echo ""
    echo "Deploying contracts..."
    forge script script/DeployLocal.s.sol:DeployLocalScript \
      --rpc-url http://localhost:8545 \
      --broadcast \
      --skip-simulation 2>&1

    # Verify deployment
    echo ""
    echo "Verifying deployment..."
    CODE=$(cast code 0x5FbDB2315678afecb367f032d93F642f64180aa3 --rpc-url http://localhost:8545 2>/dev/null)
    if [ "$CODE" = "0x" ] || [ -z "$CODE" ]; then
      echo "ERROR: Contract not deployed!"
      exit 1
    fi
    echo "Contracts verified on-chain"

    # Stop Anvil gracefully with SIGINT to trigger state dump
    echo ""
    echo "Dumping state..."
    APID=$(pgrep -x anvil)
    kill -INT $APID 2>/dev/null || true
    sleep 3

    if [ -f "$STATE_FILE" ]; then
      SIZE=$(wc -c < "$STATE_FILE" | tr -d " ")
      echo "State file created: $SIZE bytes"
    else
      echo "ERROR: State file not created!"
      exit 1
    fi
  '

if [ -f "$STATE_FILE" ]; then
  SIZE=$(wc -c < "$STATE_FILE" | tr -d ' ')
  echo ""
  echo "Success! State file: packages/contracts/anvil-state.json ($SIZE bytes)"
  echo ""
  echo "Preloaded contracts:"
  echo "  USDC Token:            0x5FbDB2315678afecb367f032d93F642f64180aa3"
  echo "  TokenNetworkRegistry:  0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512"
  echo "  TokenNetwork:          0xCafac3dD18aC6c6e92c921884f9E4176737C052c"
else
  echo "ERROR: State file generation failed"
  exit 1
fi
