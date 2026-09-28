#!/bin/sh
# =============================================================================
# The local anvil's entrypoint: start anvil, seed it, keep it in the
# foreground.
#
# What the chain holds, and why every address is fixed, is `seed.sh`'s header
# (mounted beside this file at /anvil). This file only orders the two and
# decides what a failed seed looks like.
# =============================================================================
set -eu

RPC=http://localhost:8545

echo "Starting anvil..."
anvil --host 0.0.0.0 --port 8545 --chain-id 31337 --accounts 10 --balance 10000 &
ANVIL_PID=$!

cleanup() {
  kill -TERM "$ANVIL_PID" 2>/dev/null || true
}
trap cleanup TERM INT

# Bounded: an anvil that died on start would otherwise leave this spinning with
# nothing in the log. The healthcheck never passes either way.
waited=0
until cast client --rpc-url "$RPC" 2>/dev/null | grep -q anvil; do
  waited=$((waited + 1))
  if [ "$waited" -ge 120 ] || ! kill -0 "$ANVIL_PID" 2>/dev/null; then
    echo "anvil did not answer on $RPC; not seeding." >&2
    wait "$ANVIL_PID"
    exit 1
  fi
  sleep 1
done

# The success line is CONDITIONAL. A failed seed leaves anvil serving a chain
# without the contracts every committed local config names, which the
# healthcheck in docker-compose.yml never passes -- so the log must not print
# "ready" over it. `sh -eu` runs the seed as its own process, where `set -e`
# holds; inside this `if` it would not.
if sh -eu /anvil/seed.sh; then
  echo "Anvil ready: x402 placed, USDC deployed."
else
  echo "============================================================"
  echo "SEEDING FAILED. This anvil is serving a chain WITHOUT the x402"
  echo "contracts or the USDC every committed local config names, so it"
  echo "will never pass its healthcheck. The output above is the reason;"
  echo "the container stays up so it can be read."
  echo "============================================================"
fi

# Anvil stays in the foreground either way: exiting here would let
# `restart: unless-stopped` crash-loop the container and replace a readable
# log with a scrolling one.
wait "$ANVIL_PID"
