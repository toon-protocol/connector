#!/bin/sh
# =============================================================================
# Seed the local anvil: x402's batch-settlement contracts at their canonical
# addresses, and Circle's FiatToken v2.2 as USDC (ADR 0075 decision 13).
#
# Run by `infra/anvil/entrypoint.sh` once anvil answers, as its own `sh -eu`
# process so that ANY failed step fails the whole seed (a function called from
# an `if` would run with `set -e` silently off). Every
# byte it places or deploys is the committed bytecode under
# `crates/connector-settlement-evm/contracts/` (mounted at /evm), and it is
# placed exactly the way the tier-3 tests place it
# (`connector_settlement_evm::test_support::x402::X402Chain::place`) and
# toon-protocol/infra's `sandbox/scripts/seed-x402.sh` does:
#
#   * `x402BatchSettlement`, `ERC3009DepositCollector`,
#     `Permit2DepositCollector`, Uniswap's Permit2 and Circle's
#     `SignatureChecker` library go to their canonical addresses by
#     `anvil_setCode`. None needs storage: each rebuilds its EIP-712
#     separator when `block.chainid` differs from the one it cached, and the
#     collectors' immutables are the other canonical addresses.
#   * USDC is Circle's FiatToken v2.2, DEPLOYED (implementation, then proxy)
#     and initialised, because a FiatToken's initialisers write the state that
#     makes it work. It is what gives a deposit ERC-3009's
#     `receiveWithAuthorization`. Named "USDC", version "2", 6 decimals, as
#     Base's is -- the EIP-712 domain every `[settlement.evm]` table under
#     `local/` names.
#
# USDC IS MINTED ON DEMAND, NEVER DRIPPED. Anvil's account 1 is the token's
# owner, master minter and a minter with an unlimited allowance, so
# `local/keys.sh` mints each node's USDC with `cast send ... mint(...)` from
# that account, the way it minted `MockERC20` before. No faucet is involved.
#
# ── No TOON deployment ───────────────────────────────────────────────────────
#
# Every channel is an x402 channel (ADR 0075), and the connector boots only
# if `x402BatchSettlement` is present on its chain (decision 1), so this
# chain carries nothing of TOON's -- no `TokenNetworkRegistry`, no
# `TokenNetwork`, no `MockERC20`, no ERC-2771 forwarder, no
# `RollingSwapChannel` (#1385) -- and needs neither a Solidity toolchain nor
# the contracts' git submodules.
#
# ── Deterministic addresses ──────────────────────────────────────────────────
#
# Everything is sent from anvil's genesis accounts at fixed nonces on a chain
# `--reset` on every start, so every address below is the same on every
# machine and every committed config under `local/` can name it:
#
#   account 0, nonce 0   FiatToken v2.2 implementation   0x5FbDB2315678afecb367f032d93F642f64180aa3
#   account 0, nonce 1   FiatTokenProxy -- THE USDC       0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512
#
# `crates/connector-bin/tests/local_topologies_load.rs` holds this table to
# the committed configs. The healthcheck in docker-compose.yml waits on the
# LAST step below -- account 1 becoming USDC's minter -- so a healthy anvil
# is a fully seeded one.
#
# The proxy's admin is account 0, which a transparent proxy then refuses
# every token call from; the token's roles are therefore account 1's.
# =============================================================================
set -eu

RPC=http://localhost:8545
EVM=/evm
X402="$EVM/x402"

# anvil's own published genesis keys -- "test test ... junk", printed on
# every start. Only ever pointed at this disposable chain.
DEPLOYER_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
TOKEN_OWNER_KEY=0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d
TOKEN_OWNER=0x70997970C51812dc3A010C7d01b50e0d17dc79C8

# The canonical addresses the connector binds as constants of the binary
# (`connector_signer::X402_BATCH_SETTLEMENT_ADDRESS`,
# `connector_settlement_evm::{ERC3009_DEPOSIT_COLLECTOR_ADDRESS,
# PERMIT2_DEPOSIT_COLLECTOR_ADDRESS, PERMIT2_ADDRESS}`), and the address
# FiatToken v2.2's creation code links `SignatureChecker` at.
X402_BATCH_SETTLEMENT=0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003
ERC3009_DEPOSIT_COLLECTOR=0x4020806089470a89826cB9fB1f4059150b550004
PERMIT2_DEPOSIT_COLLECTOR=0x4020425FAf3B746C082C2f942b4E5159887B0005
PERMIT2=0x000000000022D473030F116dDEE9F6B43aC78BA3
SIGNATURE_CHECKER=0xbA3b60c21e28C41df4bABd90f228e1D368627DA6

USDC=0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512

# The committed hex, `0x`-prefixed, whitespace stripped.
hex() {
  printf '0x%s' "$(tr -d ' \n\r' <"$1" | sed 's/^0x//')"
}

place() {
  cast rpc --rpc-url "$RPC" anvil_setCode "$1" "$(hex "$2")" >/dev/null
  echo "  placed $(basename "$2" .runtime.hex) at $1"
}

# Deploy `$2` (creation code, with any ABI-encoded constructor arguments
# appended) from key `$1`, and print the contract address the chain reports.
deploy() {
  cast send --rpc-url "$RPC" --private-key "$1" --json --create "$2" |
    sed -n 's/.*"contractAddress":"\(0x[0-9a-fA-F]\{40\}\)".*/\1/p'
}

# The address `$1` must equal `$2`, or the committed configs name a contract
# that is not there. Loud, because the alternative is a node refusing to start
# much later with a message that names neither address.
expect_address() {
  if [ "$(echo "$1" | tr 'A-F' 'a-f')" != "$(echo "$2" | tr 'A-F' 'a-f')" ]; then
    echo "$3 landed at '$1', not at $2, which every committed config under local/ names." >&2
    exit 1
  fi
}

echo "Placing x402's batch-settlement contracts..."
place "$SIGNATURE_CHECKER" "$X402/SignatureChecker.runtime.hex"
place "$X402_BATCH_SETTLEMENT" "$X402/x402BatchSettlement.runtime.hex"
place "$ERC3009_DEPOSIT_COLLECTOR" "$X402/ERC3009DepositCollector.runtime.hex"
place "$PERMIT2_DEPOSIT_COLLECTOR" "$X402/Permit2DepositCollector.runtime.hex"
place "$PERMIT2" "$X402/Permit2.runtime.hex"

echo "Deploying Circle's FiatToken v2.2 as USDC..."
implementation="$(deploy "$DEPLOYER_KEY" "$(hex "$X402/FiatTokenV2_2.creation.hex")")"
proxy="$(deploy "$DEPLOYER_KEY" \
  "$(hex "$X402/FiatTokenProxy.creation.hex")$(cast abi-encode 'constructor(address)' "$implementation" | sed 's/^0x//')")"
expect_address "$proxy" "$USDC" "The FiatToken proxy"

owner_send() {
  cast send --rpc-url "$RPC" --private-key "$TOKEN_OWNER_KEY" "$USDC" "$@" >/dev/null
}
owner_send 'initialize(string,string,string,uint8,address,address,address,address)' \
  USDC USDC USD 6 "$TOKEN_OWNER" "$TOKEN_OWNER" "$TOKEN_OWNER" "$TOKEN_OWNER"
owner_send 'initializeV2(string)' USDC
owner_send 'initializeV2_1(address)' "$TOKEN_OWNER"
owner_send 'initializeV2_2(address[],string)' '[]' USDC
owner_send 'configureMinter(address,uint256)' "$TOKEN_OWNER" \
  115792089237316195423570985008687907853269984665640564039457584007913129639935
echo "  USDC at $USDC (FiatToken v2.2, 6 decimals; minter $TOKEN_OWNER)"
