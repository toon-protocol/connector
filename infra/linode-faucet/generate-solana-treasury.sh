#!/usr/bin/env bash
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Generate the faucet box's fresh Solana USDC treasury keypair (issue #919,
# docs/operators/faucet-box-bringup.md step 4) and fund it with devnet SOL for
# tx fees.
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Run ON THE FAUCET BOX ITSELF (§4.4 — never copy a key generated elsewhere,
# including from box 1/the apex). This script never prints the private key: it
# writes the keypair straight to OUT_PATH (default the exact path
# docker-compose.faucet.yml bind-mounts, mode 600) and only ever echoes the
# PUBLIC key.
#
# What this script does NOT do: create the mint. Run
# ./create-devnet-usdc-mint.sh next -- it makes the keypair written here the
# MINT AUTHORITY of a fresh mock-USDC mint, which is what lets the faucet coin
# tokens per drip instead of spending a balance somebody has to top up.
#
# The treasury never needs a USDC balance at all, and that is deliberate. The
# arrangement it replaces needed one, and died of it: the mint the fleet used
# until now was created from a key that is now lost, so its treasury could
# never be refilled and the leg had no repair path.
#
#   ./generate-solana-treasury.sh [OUT_PATH] [SOL_AMOUNT] [RPC_URL]
#     OUT_PATH    default /root/keys/solana-usdc-treasury.json
#     SOL_AMOUNT  default 2 (devnet SOL, tx fees only -- this box dispenses no
#                 SOL to faucet recipients, §4.6; this SOL pays the treasury's
#                 OWN mint-tx fees and its recipients' ATA rent)
#     RPC_URL     default https://api.devnet.solana.com
#
# Requires solana-keygen + solana on PATH. The faucet box's bootstrap.sh does
# NOT install the Solana CLI (it installs docker, git, jq, gettext-base,
# openssl, ufw, curl, iptables) -- install it on the box before running this.
# Install the version docs/operators/faucet-box-bringup.md step 3 names, with
# the command it gives, rather than `stable` or a package manager's build: it
# is the one Solana CLI this repository installs anywhere, and
# crates/connector-settlement-solana/tests/solana_cli_pins.rs records why.
# Deliberately no version literal here -- the runbook holds the single copy so
# the two cannot drift apart.
set -euo pipefail

OUT_PATH="${1:-/root/keys/solana-usdc-treasury.json}"
SOL_AMOUNT="${2:-2}"
RPC_URL="${3:-https://api.devnet.solana.com}"

for bin in solana-keygen solana; do
  command -v "$bin" >/dev/null 2>&1 || {
    echo "Error: $bin not found on PATH." >&2
    exit 1
  }
done

# Idempotent-safe: never clobber an existing treasury. Re-running this script
# against a live box must not be able to orphan an already-funded key.
if [ -e "$OUT_PATH" ]; then
  echo "Error: $OUT_PATH already exists -- refusing to overwrite a possibly-funded treasury." >&2
  echo "Remove it first if you really mean to generate a new one." >&2
  exit 1
fi

mkdir -p "$(dirname "$OUT_PATH")"

echo "==> Generating a fresh Solana keypair at $OUT_PATH"
solana-keygen new --no-bip39-passphrase --silent --outfile "$OUT_PATH"
chmod 600 "$OUT_PATH"

PUBKEY="$(solana-keygen pubkey "$OUT_PATH")"
echo "==> Treasury public key: $PUBKEY"

echo "==> Airdropping $SOL_AMOUNT SOL (tx fees) via $RPC_URL"
if solana airdrop "$SOL_AMOUNT" "$PUBKEY" --url "$RPC_URL"; then
  echo "    Airdrop succeeded."
else
  echo "    Airdrop failed (the devnet airdrop is rate-limited per IP/address, and the" >&2
  echo "    RPC itself can be unreachable) -- retry" >&2
  echo "    later with: solana airdrop $SOL_AMOUNT $PUBKEY --url $RPC_URL" >&2
fi

cat <<EOF

==> Next steps (not done by this script):
    1. Create this box's mint, with $PUBKEY as its authority:
         ./create-devnet-usdc-mint.sh
       No USDC funding step exists or is needed -- the faucet mints per drip.
    2. Put that script's mint address in this box's .env as SOLANA_USDC_MINT,
       then restart the faucet service. Only if $OUT_PATH is not the default
       /root/keys/solana-usdc-treasury.json, repoint
       docker-compose.faucet.yml's bind mount at it first.
    3. Verify: POST /api/solana/usdc-request against this box and confirm the
       recipient's USDC balance rises (faucet-box-bringup.md gate (c)).

The private key was written to $OUT_PATH and was never printed by this script.
EOF
