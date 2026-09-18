#!/usr/bin/env bash
#
# regen-anvil-state.sh — deterministically regenerate packages/contracts/anvil-state.json
#
# WHY THIS EXISTS
# ---------------
# anvil's `--dump-state` JSON schema is versioned and changes across foundry
# releases (e.g. anvil 1.7.1 made `address` required on trace log entries), so a
# snapshot dumped by an older anvil eventually fails to `--load-state` under a
# newer one:
#
#   error: invalid value '…/anvil-state.json' for '--load-state <PATH>':
#          failed to parse json file: missing field `address`
#
# This script re-runs the canonical local deploy (DeployLocal.s.sol) against a
# fresh anvil and dumps a snapshot in the CURRENT anvil's format. Because the
# deploy is deterministic (fixed deployer key + nonces), the contract addresses
# are stable across regenerations:
#
#   USDC (MockERC20)      0x5FbDB2315678afecb367f032d93F642f64180aa3
#   TokenNetworkRegistry  0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512
#   ERC2771Forwarder      0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0
#   TokenNetwork (USDC)   0xCafac3dd18aC6c6e92c921884f9E4176737C052c
#   RollingSwapChannel    0x5FC8d32690cc91D4c39d9d3abcBD16989F875707
#
# The forwarder (issue #1261) is deployed AFTER the registry, which is why the
# three addresses above it are the same as before it existed and only
# RollingSwapChannel moved. DeployLocal.s.sol's docblock carries why, and
# test/DeployLocal.t.sol holds the script to it.
#
# HOW TO REFRESH AFTER A FOUNDRY BUMP
# -----------------------------------
#   cd packages/contracts && ./regen-anvil-state.sh
#   git add anvil-state.json && git commit
#
# Requires: foundry (anvil, cast, forge) on PATH. Run from packages/contracts.
set -euo pipefail

cd "$(dirname "$0")"

PORT="${ANVIL_PORT:-8555}"
RPC="http://127.0.0.1:${PORT}"
STATE_FILE="anvil-state.json"

echo "anvil version: $(anvil --version | head -1)"

# forge-std + openzeppelin come from git submodules; make the deploy self-healing
# for fresh worktrees (mirrors docker-compose.yml's anvil init, see issue #104).
if [ ! -f lib/forge-std/src/Script.sol ] || [ ! -f lib/openzeppelin-contracts/contracts/access/Ownable.sol ]; then
  echo "Foundry libs missing — initializing submodules..."
  git submodule update --init --recursive lib/forge-std lib/openzeppelin-contracts \
    || forge install foundry-rs/forge-std OpenZeppelin/openzeppelin-contracts --no-git
fi

# Start a fresh anvil that will dump its state to STATE_FILE on graceful exit.
# Fixed chain-id/accounts/balance match docker-compose.yml so the snapshot is a
# drop-in for `anvil --load-state`.
echo "Starting anvil on :${PORT} (dump-state → ${STATE_FILE})..."
anvil --host 127.0.0.1 --port "${PORT}" --chain-id 31337 --accounts 10 --balance 10000 \
      --dump-state "${STATE_FILE}" &
ANVIL_PID=$!
trap 'kill "${ANVIL_PID}" 2>/dev/null || true' EXIT

echo "Waiting for anvil to be ready..."
until cast client --rpc-url "${RPC}" 2>/dev/null | grep -q 'anvil'; do
  sleep 1
done

echo "Deploying local contracts (DeployLocal.s.sol)..."
forge script script/DeployLocal.s.sol:DeployLocalScript \
  --rpc-url "${RPC}" --broadcast --skip-simulation

echo "Stopping anvil to flush state snapshot..."
kill "${ANVIL_PID}" 2>/dev/null || true
# anvil writes the snapshot on SIGTERM; give it a moment to flush.
wait "${ANVIL_PID}" 2>/dev/null || true
trap - EXIT

# Sanity-check: the freshly dumped snapshot must load under the SAME anvil.
echo "Verifying snapshot loads under $(anvil --version | head -1)..."
anvil --load-state "${STATE_FILE}" --port "$((PORT + 1))" --chain-id 31337 &
VERIFY_PID=$!
trap 'kill "${VERIFY_PID}" 2>/dev/null || true' EXIT
sleep 3
if kill -0 "${VERIFY_PID}" 2>/dev/null; then
  echo "✅ ${STATE_FILE} regenerated and loads cleanly."
else
  echo "❌ ${STATE_FILE} failed to load — regeneration produced a bad snapshot." >&2
  exit 1
fi
kill "${VERIFY_PID}" 2>/dev/null || true
trap - EXIT
