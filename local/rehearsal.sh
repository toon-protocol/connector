# shellcheck shell=sh
# =============================================================================
# What every peered topology's `sender` asserts, once, in the image's own
# busybox sh. Sourced by the `sender` service of `local/<topology>/compose.yml`
# (mounted at /app/rehearsal.sh); never run on its own.
#
# The exit status of `connector send --expect-fulfill` is not enough on a
# peering, and this file is the rest. A voucher's verdict rides back in the ack
# and never gates the packet, so a peering whose every voucher was refused
# still FULFILLs; and a voucher is checked against the signer the chain records
# for its channel, not against whether the collateral behind it is still
# there. So two more questions are asked, of two independent witnesses:
#
#   * THE CHAIN: is the payer's channel real, in the program or contract every
#     channel lives in (ADR 0075), and does it still hold the collateral the
#     payer put behind it?
#   * THE PAYEE'S OWN JOURNAL: did the payee accept exactly one voucher per
#     crossing on that channel, each advancing its watermark by exactly what
#     the hop before it forwarded?
#
# The channel ids come from `local/keys.sh <topology> channels`, which wrote
# each peering's key -- `<chain>:<channel>`, the spelling the payee journals
# it under -- into `/app/peerings/<id>`. They are not committed: an x402
# channel is opened with a fresh salt, so its id is a fact of this run.
# =============================================================================

# The channel key the payee journals peering `$1`'s vouchers under.
peering_key() {
  if [ ! -s "/app/peerings/$1" ]; then
    echo "NO PEERING: /app/peerings/$1 is missing, so 'local/keys.sh <topology> channels'" >&2
    echo "            never established the '$1' peering on this run." >&2
    exit 1
  fi
  cat "/app/peerings/$1"
}

# `$1` JSON-RPC body, posted to `$2`, answer on stdout. `content-type` named
# because busybox wget sends `application/x-www-form-urlencoded` for a
# --post-data body, and the Solana validator answers that with 415.
rpc() {
  wget -q -O - --header 'content-type: application/json' --post-data "$1" "$2"
}

# The EVM channel `$1` (`evm:0x...`) holds at least `$2` base units in
# `x402BatchSettlement` itself: the contract's own `channels(id)`, whose first
# word is the channel's balance. `$3` names the peering in a failure.
evm_channel_holds() {
  id=${1#evm:}
  answer=$(rpc "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"eth_call\",\"params\":[{\"to\":\"0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003\",\"data\":\"0x7a7ebd7b${id#0x}\"},\"latest\"]}" \
    http://anvil:8545) || {
    echo "NO CHANNEL: eth_call channels($id) failed against the local anvil." >&2
    exit 1
  }
  word=$(echo "$answer" | sed -n 's/.*"result":"0x\([0-9a-f]\{64\}\).*/\1/p')
  # The balance is a uint128 in the low half of the first word; a local
  # deposit fits in the low 15 hex digits, which busybox arithmetic reads --
  # and anything in the high digits is refused rather than truncated.
  held=""
  if [ -n "$word" ] && [ -z "$(echo "$word" | cut -c1-49 | tr -d 0)" ]; then
    held=$((0x$(echo "$word" | cut -c50-64)))
  fi
  if [ -z "$held" ] || [ "$held" -lt "$2" ]; then
    echo "NO COLLATERAL: x402BatchSettlement holds ${held:-nothing} in $id, not the $2 the '$3'" >&2
    echo "               payer was topped up to. A voucher on an unfunded channel is a" >&2
    echo "               signature, not a payment: it could never be landed." >&2
    echo "               answer: $answer" >&2
    exit 1
  fi
  echo "--- '$3': x402BatchSettlement holds $held in $id (at least $2)"
}

# The Solana channel `$1` (`solana:<account>`) is an account of
# `payment-channels` holding at least `$2` in its `deposit` (the u64 at offset
# 12 of the 256-byte `Channel`, `connector_settlement_solana::batch::wire`).
# `commitment: confirmed`, named rather than defaulted: getAccountInfo defaults
# to FINALIZED, which a channel funded seconds ago by `make local-up` is not
# yet, and confirmed is the level the backend itself reads and writes at.
solana_channel_holds() {
  account=${1#solana:}
  if ! rpc "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getAccountInfo\",\"params\":[\"$account\",{\"encoding\":\"base64\",\"commitment\":\"confirmed\"}]}" \
    http://solana-validator:8899 >/tmp/channel.json; then
    echo "NO CHANNEL: getAccountInfo for $account failed against the local validator." >&2
    exit 1
  fi
  if ! grep -q '"owner":"CHNLxYvVA28MJP9PrFuDXccuoGXAx7jBacfLEkahyGsX"' /tmp/channel.json; then
    echo "NO CHANNEL: $account is not an account of payment-channels on the local validator." >&2
    cat /tmp/channel.json >&2
    exit 1
  fi
  sed 's/.*"data":\["\([^"]*\)".*/\1/' /tmp/channel.json | base64 -d >/tmp/channel.bin
  held=$(od -An -tu8 -j12 -N8 /tmp/channel.bin | tr -d ' ')
  if [ -z "$held" ] || [ "$held" -lt "$2" ]; then
    echo "NO COLLATERAL: $account holds ${held:-nothing} base units, not the $2 the '$3' payer" >&2
    echo "               was topped up to." >&2
    exit 1
  fi
  echo "--- '$3': payment-channels account $account holds $held (at least $2)"
}

# Either chain, by the key's own prefix.
channel_holds() {
  case "$1" in
    evm:*) evm_channel_holds "$@" ;;
    solana:*) solana_channel_holds "$@" ;;
    *)
      echo "UNKNOWN CHANNEL KEY '$1' for '$3'." >&2
      exit 1
      ;;
  esac
}

# `paid <state dir> <channel key> <label> <crossings> <advance>`: the payee
# whose state is at `$1` accepted exactly `$4` vouchers on `$2`, each
# advancing the channel's watermark by exactly `$5`.
#
# One line per accepted voucher, in the order the client-edge book wrote them
# -- `inbound_claim_accepted <key> <nonce> <cumulative> <signature>`, tab
# separated (`connector_runtime::journal`) -- in `client-edge-claims.log`, the
# one book an x402 channel keeps whichever role its vouchers arrive under.
#
# EXACTLY, both counts. Fewer vouchers than crossings is a crossing carried for
# free. An advance below the figure is a hop keeping more than its fee, above
# it a hop keeping less -- ADR 0010's earnings rule, "the difference between
# the cumulative it receives from upstream and the cumulative it sends
# downstream", measured on a running image; and on `dealing` it is the
# CONVERTED figure, which a boundary at the wrong rate moves while fulfilling
# every packet just as happily. An advance of zero is issue #1102's defect: a
# payer whose watermark was restored from the wrong place, re-signing one
# cumulative amount forever.
paid() {
  journal="$1/client-edge-claims.log"
  if [ ! -f "$journal" ]; then
    echo "NOT PAID: $3 kept no voucher journal at $journal, so nothing was ever accepted" >&2
    echo "          and every crossing was carried for free." >&2
    exit 1
  fi
  echo "--- what $3 was actually paid on $2, out of its own journal"
  if ! awk -F '\t' -v key="$2" -v crossings="$4" -v advance="$5" '
      $1 == "inbound_claim_accepted" && $2 == key {
        accepted = accepted + 1
        step = $4 - cumulative
        printf "    voucher %d: cumulative %s, advance %d\n", accepted, $4, step
        if (step != advance) {
          printf "    ^^ that advance is not the %d one crossing owes\n", advance
          wrong = 1
        }
        cumulative = $4
      }
      END {
        if (accepted != crossings) {
          printf "%d vouchers accepted on this channel for %d crossings\n", accepted, crossings
          exit 1
        }
        if (wrong) { exit 1 }
        printf "    watermark %d = %d crossings x %d\n", cumulative, crossings, advance
      }
    ' "$journal"; then
    echo "NOT PAID: $3 was not paid for every crossing it carried -- see the lines above." >&2
    echo "--- $journal follows ---" >&2
    cat "$journal" >&2
    exit 1
  fi
}
