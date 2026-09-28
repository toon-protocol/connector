#!/bin/sh
# Solana Test Validator Entrypoint
#
# Loads solana-foundation's `payment-channels` and mainnet's p-token (below)
# into GENESIS at their fixed program ids and starts the validator. Nothing is
# deployed after startup and no keypair is needed for any of it: passing a
# program to `--bpf-program` under a bare id at genesis is what the Rust test
# harness does too (`connector_settlement_solana::test_support::
# SolanaValidator::spawn`), so both tiers load the same bytes at the same ids.
#
# TOON's own payment-channel program is no longer loaded: every channel is an
# x402 channel on `payment-channels` (ADR 0075), and the connector no longer
# boots through `[settlement.solana] program_id` (#1385).
set -eu

# Explicit ledger dir rather than the default ./test-ledger relative to CWD.
LEDGER_DIR=/workspace/test-ledger

# Trap SIGTERM/SIGINT and forward to the validator for graceful shutdown
cleanup() {
  if [ -n "${VALIDATOR_PID:-}" ]; then
    kill -TERM "$VALIDATOR_PID" 2>/dev/null || true
  fi
}
trap cleanup TERM INT

# ── The programs a channel actually lives on (ADR 0075 decision 13) ──────────
# solana-foundation's `payment-channels` at its canonical id, and mainnet-beta's
# Token program (p-token) at the SPL Token id, from the SAME pinned fixtures the
# in-process harness loads (`connector_settlement_solana::test_support::
# SolanaValidator::spawn`, mounted read-only from
# crates/connector-settlement-solana/fixtures/, whose hashes a test pins). p-token
# replaces the bundled SPL Token because the latter refuses the `Batch` a
# two-payout `distribute` sends (#1358); it is a drop-in for every instruction
# `spl-token` and `infra/solana/create-usdc-mint.sh` send.
#
# REQUIRED: a validator without them cannot hold a single local channel, and a
# connector refuses to boot on a chain without `payment-channels` (ADR 0075
# decision 1). The fixtures are committed, so their absence means a broken
# mount rather than an unbuilt tree.
PAYMENT_CHANNELS_ID=CHNLxYvVA28MJP9PrFuDXccuoGXAx7jBacfLEkahyGsX
SPL_TOKEN=TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA
for fixture in /fixtures/payment_channels.so /fixtures/p_token.so; do
  if [ ! -f "$fixture" ]; then
    echo "ERROR: $fixture is missing -- crates/connector-settlement-solana/fixtures is not mounted." >&2
    exit 1
  fi
done
echo "Loading payment-channels at $PAYMENT_CHANNELS_ID and p-token at $SPL_TOKEN"
set -- \
  --bpf-program "$PAYMENT_CHANNELS_ID" /fixtures/payment_channels.so \
  --bpf-program "$SPL_TOKEN" /fixtures/p_token.so

# --limit-ledger-size caps how many shreds the rocksdb ledger retains. NOTE the
# `solana-test-validator` default is only 10000 shreds (NOT the full validator's
# 200,000,000) -- so the old explicit 50,000,000 here was a ~5000x override that
# let rocksdb grow to ~63 GB in ~21h and fill the 80 GB devnet box's disk. When
# the disk is full the validator silently STOPS producing blocks (slot freezes)
# while /health still returns "ok", so faucet/settlement writes hang then 500.
# 10,000,000 shreds bounds the ledger to ~12-13 GB (~1.26 KB/shred observed) --
# generous recent history for claim verification, with wide headroom on disk.
# Verified accepted by this image's validator (agave 4.0.3, ghcr.io/beeman/
# solana-test-validator): `--limit-ledger-size` defaults to only 10000 shreds and
# enforces NO 50M minimum (that floor is the full `solana-validator`, not the
# test validator), so 10,000,000 starts cleanly -- no crash-loop risk.
solana-test-validator --reset --ledger "$LEDGER_DIR" --limit-ledger-size 10000000 "$@" &
VALIDATOR_PID=$!

echo "Waiting for Solana validator to be ready..."
until solana cluster-version --url http://localhost:8899 2>/dev/null; do
  sleep 1
done

echo "Solana validator ready."
wait $VALIDATOR_PID
