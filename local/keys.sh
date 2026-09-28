#!/usr/bin/env bash
# =============================================================================
# Provision the key material a local topology needs, fund it, and -- once the
# nodes are serving -- establish its peerings.
#
#   local/keys.sh <topology>            # e.g. local/keys.sh solo (pre-boot)
#   local/keys.sh <topology> channels   # after the nodes are serving
#
# Everything lands in local/.keys/<topology>/, which is GITIGNORED. Nothing this
# script writes is ever committed: ADR 0012 makes key material a location
# rather than a value, and every committed connector.toml under local/ names
# these as paths.
#
# One directory per NODE, named after that node's compose service, and the
# node's config file is `local/<topology>/<node>.toml`. A topology with one
# node (`solo`) is not a special case of that -- it is the same rule with one
# entry.
#
# No faucet is involved on either chain. The faucet is an app-layer service and
# is not part of the connector; local chains fund from genesis, and USDC is
# minted on demand.
#
# Idempotent. Re-running keeps existing keys, re-funds them, and finds an
# already-established peering rather than opening a second channel -- which is
# the common case: both local chains wipe their state on every start, so the
# accounts survive in this directory while their balances do not.
#
# ── Every channel is an x402 channel (ADR 0075) ──────────────────────────────
#
# A peering is TWO one-way `batch-settlement` channels, and each node opens and
# funds only its OWN outbound one. Nothing here opens a channel with a chain
# CLI: the `channels` stage sends each node the signed operator writes an
# operator would (ADR 0008) --
#
#   POST /peers         on both ends of every peering (ADR 0058 as ADR 0075
#                       decision 4 amends it): read the other's
#                       self-description, open this node's channel toward it
#                       (on Solana by posting the payer-signed `open` to the
#                       other's sponsor endpoint, decision 3), and bind the
#                       other's channel by the voucher signer it publishes;
#   POST /channels/:id/fund
#                       top the paying side's channel up to its target (an
#                       INCREMENT, so the shortfall is read first);
#   POST /routes/peers  point the payer's forwarding prefix at the peering.
#
# -- and then reads every channel back off the chain it lives on, refusing to
# report success unless the chain agrees. Nothing on the packet path makes that
# check for it: a voucher is verified against the signer the chain records for
# its channel, but whether the collateral behind it is still there is a
# question only the chain answers, and a topology whose channel was never
# funded would otherwise rehearse exactly as green as one whose channel is.
#
# ── Which keys are RANDOM and which are DERIVED ──────────────────────────────
#
#   * `signer.key`, `operator-send.key` and `operator-bearer-token` are
#     RANDOM. None appears in a committed file.
#
#   * `settlement.key` and `settlement-solana.key` are DERIVED, per node, from
#     anvil's own published test mnemonic at a fixed index. No committed file
#     names the addresses any more -- a peering binds the other side's channel
#     by the voucher signer its self-description publishes, so there is no
#     `counterparty_key` to write down -- but a settlement address that is the
#     same on every machine and every run is still one fewer thing to
#     disambiguate when a log names it, and the `channels` stage checks every
#     channel's counterparty against it.
#
# The mnemonic is public knowledge -- anvil prints it on every start -- so
# deriving from it introduces no secret that did not already exist. EVM and
# Solana take DISJOINT index ranges, so no 32 bytes is ever used on both
# curves. The Solana derivation is not BIP44-for-Solana and does not claim to
# be: `[settlement.solana.key]` is a raw 32-byte SEED handed to
# `keypair_from_seed`, and a BIP32 derivation is just a deterministic 32-byte
# function of (mnemonic, index), so it supplies one.
# =============================================================================
set -euo pipefail

TOPOLOGY="${1:-}"
# Two stages, because they cannot run at the same moment. `keys` is everything
# that has to exist BEFORE a node starts -- the key files it mounts and their
# funding on both chains. `channels` is what can only happen AFTER: every
# channel is opened by a running node's own signed write, because a channel's
# payer is that node's settlement key and nothing else holds it. `make
# local-up` calls both, in that order, with the connectors started in between.
STAGE="${2:-keys}"
if [[ -z "$TOPOLOGY" ]]; then
  echo "usage: local/keys.sh <topology> [stage]" >&2
  echo "       topology: solo, two-hop, mixed-chain, onion, dealing" >&2
  echo "       stage:    keys (default, pre-boot) | channels (post-boot)" >&2
  exit 1
fi
case "$STAGE" in
  keys | channels) ;;
  solana-channels)
    echo "ERROR: the 'solana-channels' stage is now 'channels' (ADR 0075): every peering, on" >&2
    echo "       both chains, is established by the running nodes' own POST /peers." >&2
    exit 1
    ;;
  *)
    echo "ERROR: unknown stage '$STAGE'. Known stages: keys, channels." >&2
    exit 1
    ;;
esac

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
KEYS="$HERE/.keys/$TOPOLOGY"
CONNECTOR="$ROOT/target/release/connector"
SIGN_WRITE="$ROOT/docs/operators/sign-write.sh"

ANVIL_RPC="${ANVIL_RPC:-http://127.0.0.1:8545}"
SOLANA_RPC="${SOLANA_RPC:-http://127.0.0.1:8899}"

# anvil's own published accounts and the mnemonic they derive from -- "test
# test ... junk", public knowledge, printed by anvil on every start. Only ever
# pointed at a disposable local chain.
#
# Account 0 funds every node's ETH. Account 1 is the local USDC's owner and
# minter (`infra/anvil/seed.sh`), so every node's USDC is a MINT from it --
# minted on demand, never dripped out of somebody's balance.
ANVIL_MNEMONIC="test test test test test test test test test test test junk"
ANVIL_ACCOUNT0_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
USDC_MINTER_KEY=0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d

# The addresses `infra/anvil/seed.sh` places and deploys at, which every
# committed config under local/ names: Circle's FiatToken v2.2 as USDC, and the
# canonical `x402BatchSettlement` every EVM channel lives in.
USDC=0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512
X402_BATCH_SETTLEMENT=0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003

# The deterministic mock mint (`infra/solana/create-usdc-mint.sh`) and the
# program every Solana channel lives in: solana-foundation's `payment-channels`,
# loaded at genesis by `infra/solana/entrypoint.sh`.
SOLANA_USDC_MINT=H8HSreUF2s8r8hem4qMttE3bWYCpFuh71jbuos5bA77H
PAYMENT_CHANNELS_PROGRAM=CHNLxYvVA28MJP9PrFuDXccuoGXAx7jBacfLEkahyGsX

# The mock mint's authority, and the only holder of any of its supply until
# this script hands some out: `infra/solana/create-usdc-mint.sh` mints 100M to
# this keypair's own associated token account and distributes none of it. It is
# a real, spendable key committed to this repository on purpose -- it signs for
# a token that exists only on a disposable local validator, that script refuses
# any RPC URL naming mainnet, and `tools/ci/check-tracked-secrets.sh`
# allowlists exactly this path with that reason. Referenced here; never copied,
# never printed.
SOLANA_USDC_AUTHORITY="$ROOT/infra/solana/usdc-authority.json"

# ── The dealing topology's second mock token ─────────────────────────────────
# `local/dealing` crosses a real DENOMINATION boundary (ADR 0071), and a
# boundary needs two tokens that genuinely differ. USDC-on-anvil and
# USDC-on-the-validator are two `AssetId`s, but the same asset at the same
# scale, and a crossing at par cannot tell a correct conversion from no
# conversion at all. A thousandfold scale difference can, so that leg settles
# in a NINE-decimal mock instead, created below.
#
# Its KEYPAIR is derived, at a fixed index of the same public anvil mnemonic:
# the ADDRESS is committed in `local/dealing/connector-{b,c}.toml` (and in B's
# `[[tokens]]`/`[[rates]]`) and has to be identical on every machine and after
# every `--reset`, while nothing secret may be written down. Its AUTHORITY is
# the committed `usdc-authority.json` above -- one allowlisted local-chain key,
# not a second one.
DEALING_MINT_INDEX=28
DEALING_MINT_DECIMALS=9
# A UI amount, like every other `spl-token` figure.
DEALING_MINT_TREASURY=100000000

# What a peering's PAYER ends up with behind its outbound channel, in 6-decimal
# USDC base units: 100 USDC against a crossing of about 1000 µUSDC, so a
# topology can be rehearsed a hundred thousand times before collateral is the
# reason something fails.
#
# It gets there in two writes, deliberately: `POST /peers` opens the channel
# with OPEN_DEPOSIT, and `POST /channels/:id/fund` tops it up by the shortfall
# -- read off the node's own `GET /channels` first, because `fund` takes an
# INCREMENT. So the funding endpoint is exercised on every bring-up, and a
# second `make local-up` moves no money at all.
CHANNEL_DEPOSIT=100000000
# The same 100 tokens on a leg that settles in the 9-decimal mock -- a thousand
# times the base units, because base units are what a deposit is denominated
# in (ADR 0071 seen from the collateral side).
DEALING_CHANNEL_DEPOSIT=100000000000
# What every channel is OPENED with, payer and payee alike, in the base units
# of the channel's token. The payee's channel toward the payer -- the other
# half of every peering -- stays at this: no packet flows that way here.
# At least every local node's `min_sponsored_deposit`, which is what bounds an
# opening deposit on Solana.
OPEN_DEPOSIT=1000000

# What each node is given of the topology's tokens: 1000 USDC on anvil (in
# base units, for `mint`), and 1000 of the Solana mint as a UI amount (for
# `spl-token`, which takes UI amounts -- scale-free, so one figure serves a
# 6-decimal and a 9-decimal mint alike). Ten times CHANNEL_DEPOSIT either way.
NODE_EVM_USDC=1000000000
NODE_SOLANA_TOKENS=1000

# ── The onion topology's placeholder addresses ───────────────────────────────
# `local/onion/*.toml` are committed with these hosts wherever a daemon's real
# address goes, and this script substitutes each daemon's address for its own
# placeholder (see `render_onion_configs`). Each is 56 characters of the base32
# alphabet a v3 hidden-service address is drawn from and ends in `.anyone`, so
# the committed files load through the real parser -- which is the whole
# reason they are committed rather than generated.
#
# TWO addresses, one per node: a peering is established from BOTH ends (ADR
# 0075 decision 4), so B has to read A's self-description as well as A reading
# B's, and on this topology the only way either reaches the other is a circuit.
#
# `.anyone`, not `.onion`, because that is the TLD the daemon `local/anon-image`
# pins writes (issue #1284). `local_topologies_load.rs` holds these literals and
# the committed files to one string each.
ONION_PLACEHOLDER_A=placeholderplaceholderplaceholderplaceholderplaceholdera.anyone
ONION_PLACEHOLDER_B=placeholderplaceholderplaceholderplaceholderplaceholderb.anyone

# ── The topology table ───────────────────────────────────────────────────────
#
#   NODES     `node:evm_index:solana_index:port` -- `port` is the host port
#             compose publishes the node's client edge on, which is where the
#             `channels` stage sends its writes; `-` for a node with no host
#             port at all (onion's B, whose only door is the circuit), which is
#             written to from inside its own container instead.
#   PEERINGS  `id:chain:payer:payee:fee:max_packet_amount` -- the PAYER is the
#             side packets flow away from; `fee` is what it retains per packet
#             carried over this peering (ADR 0010, ADR 0061) and
#             `max_packet_amount` its cap in the OUTGOING leg's unit (ADR
#             0049; 0 keeps the default).
#   ROUTES    `node:prefix:peer_id:price` -- a forwarding route written with
#             `POST /routes/peers`, whose `price` is what that node's client
#             edge charges for the prefix (ADR 0028).
#
# These figures are the peering half of each topology's arithmetic, the other
# half being the committed `price` of the route that terminates it.
# `local_topologies_load.rs` parses this table and holds the two halves and the
# rehearsal's own figures to one sum.
#
# Indices are disjoint across topologies as well as across nodes, and EVM and
# Solana never share one. What the rule asks for is disjointness, not
# contiguity.
#
# A topology may also own a MINT -- `TOPOLOGY_MINT_INDEX`, a third derivation
# from the same mnemonic, for a topology that settles on Solana in a token the
# shared mock USDC mint is not. Empty for every topology but `dealing`.
TOPOLOGY_MINT_INDEX=""
TOPOLOGY_MINT_DECIMALS=6
TOPOLOGY_MINT_TREASURY=""
ROUTES=""
case "$TOPOLOGY" in
  solo)
    NODES="connector:4:14:3000"
    PEERINGS=""
    ;;
  two-hop)
    NODES="connector-a:5:15:3001 connector-b:6:16:3002"
    PEERINGS="a-b:evm:connector-a:connector-b:100:0"
    ROUTES="connector-a:g.local.two-hop.b:a-b:1100"
    ;;
  mixed-chain)
    NODES="connector-a:7:17:3003 connector-b:8:18:3004 connector-c:9:19:3005"
    PEERINGS="a-b:evm:connector-a:connector-b:100:0 b-c:solana:connector-b:connector-c:50:0"
    ROUTES="connector-a:g.local.mixed.b:a-b:1200 connector-b:g.local.mixed.b.c:b-c:1100"
    ;;
  onion)
    NODES="connector-a:10:20:3006 connector-b:11:21:-"
    PEERINGS="a-b:evm:connector-a:connector-b:200:0"
    ROUTES="connector-a:g.local.onion.b:a-b:1200"
    ;;
  dealing)
    NODES="connector-a:22:25:3007 connector-b:23:26:3008 connector-c:24:27:3009"
    # `b-c`'s fee is 6000 in the OUTGOING leg's 9-decimal unit (ADR 0071
    # decision 1), and its cap is written out because the default -- one USDC
    # at six decimals -- is checked against the converted amount and would
    # refuse every crossing `T04`.
    PEERINGS="a-b:evm:connector-a:connector-b:100:0 b-c:solana:connector-b:connector-c:6000:1000000000"
    ROUTES="connector-a:g.local.dealing.b:a-b:1200 connector-b:g.local.dealing.b.c:b-c:1100"
    TOPOLOGY_MINT_INDEX="$DEALING_MINT_INDEX"
    TOPOLOGY_MINT_DECIMALS="$DEALING_MINT_DECIMALS"
    TOPOLOGY_MINT_TREASURY="$DEALING_MINT_TREASURY"
    SOLANA_CHANNEL_DEPOSIT="$DEALING_CHANNEL_DEPOSIT"
    ;;
  *)
    echo "ERROR: unknown topology '$TOPOLOGY'." >&2
    echo "       Known topologies are the directories under local/: solo, two-hop, mixed-chain," >&2
    echo "       onion, dealing." >&2
    echo "       A new one needs an entry in this script's topology table as well as a" >&2
    echo "       directory -- keys are provisioned from the table, not discovered." >&2
    exit 1
    ;;
esac

# What a SOLANA leg's payer ends up with, which differs from the EVM figure
# only where the two legs hold differently-scaled tokens.
SOLANA_CHANNEL_DEPOSIT="${SOLANA_CHANNEL_DEPOSIT:-$CHANNEL_DEPOSIT}"

need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "ERROR: '$1' is not on PATH. $2" >&2
    exit 1
  }
}

need cast "Install Foundry: https://getfoundry.sh"
need solana "Install the Solana CLI: https://solana.com/docs/intro/installation"
need solana-keygen "Ships with the Solana CLI."
# A missing SPL CLI must stop this and say so, never leave the Solana
# settlement accounts silently tokenless (ADR 0007's rule for a missing chain
# binary).
need spl-token "Ships with the Solana CLI; otherwise 'cargo install spl-token-cli'."
need openssl "openssl generates the key material and signs every operator write."
need python3 "python3 does the encodings and JSON reads this script cannot ask a chain tool for."
need curl "curl sends the channels stage's operator writes."

if [[ ! -x "$CONNECTOR" ]]; then
  echo "ERROR: $CONNECTOR is missing. Run 'cargo build --release -p connector' first --" >&2
  echo "       this script derives the operator allowlist value with it, so the value in" >&2
  echo "       write_keys cannot disagree with whatever actually signs. It derives every" >&2
  echo "       Solana settlement PUBLIC key with it too, for the same reason." >&2
  exit 1
fi

# base58, for the one direction no tool here offers: an ed25519 public key in
# hex is what the connector prints, and base58 is what Solana spells it in.
base58() {
  python3 - "$1" <<'PY'
import sys
raw = bytes.fromhex(sys.argv[1])
alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
value = int.from_bytes(raw, "big")
encoded = ""
while value:
    value, remainder = divmod(value, 58)
    encoded = alphabet[remainder] + encoded
leading_zeros = len(raw) - len(raw.lstrip(b"\0"))
print(alphabet[0] * leading_zeros + encoded)
PY
}

# The Solana CLI's own keypair file for a raw 32-byte seed: a 64-element array
# of `seed || public key`. The public half is derived by the SAME binary that
# will sign with it, so an account this script funds -- or a mint it creates --
# is provably the account the connector knows.
solana_cli_keypair() {
  local seed_file="$1" out="$2"
  python3 - "$seed_file" "$("$CONNECTOR" send --operator-key "$seed_file" --print-keyid)" "$out" <<'PY'
import json
import sys

seed = bytes.fromhex(open(sys.argv[1]).read().strip())
public = bytes.fromhex(sys.argv[2])
assert len(seed) == 32 and len(public) == 32, "a Solana keypair is 32 bytes of seed and 32 of key"
json.dump(list(seed + public), open(sys.argv[3], "w"))
PY
  chmod 600 "$out"
}

# One field of a JSON document on stdin, by a Python expression over `doc`.
# Empty output -- never a traceback -- when the document is not JSON or the
# field is absent, so a caller's own check names what was missing.
json_field() {
  python3 -c '
import json, sys
try:
    doc = json.load(sys.stdin)
    value = eval(sys.argv[1], {"doc": doc})
except Exception:
    sys.exit(0)
if value is not None:
    print(value)
' "$1"
}

# The node's directory, its committed config, and its row of the table.
node_dir() { echo "$KEYS/$1"; }
node_config() { echo "$HERE/$TOPOLOGY/$1.toml"; }
node_entry() {
  local entry
  for entry in $NODES; do
    [[ "${entry%%:*}" == "$1" ]] && echo "$entry" && return
  done
  echo "ERROR: '$1' is not a node of '$TOPOLOGY' in this script's topology table." >&2
  exit 1
}
node_port() { echo "$(node_entry "$1")" | cut -d: -f4; }

# The private key derived for a node, on either chain. `cast wallet
# private-key` prints `0x`-prefixed; every key_file in this repository takes
# bare 64 hex, so the prefix is stripped exactly once, here.
derived_key() {
  cast wallet private-key --mnemonic "$ANVIL_MNEMONIC" --mnemonic-index "$1" | sed 's/^0x//'
}

# Assert `$2` (a committed config) names `$1` -- the drift guard for the one
# derived address still committed here, the dealing topology's mint.
config_must_name() {
  local value="$1" config="$2" what="$3"
  if ! grep -qF "$value" "$config"; then
    echo "ERROR: $config does not name $what" >&2
    echo "         $value" >&2
    echo "       That value is a deterministic function of the local chains and this" >&2
    echo "       script's own topology table, so a mismatch means the committed config is" >&2
    echo "       stale -- update it to the value above rather than editing this script." >&2
    exit 1
  fi
}

# A node's settlement identities, derived by the binary that signs with them:
# the EVM address (lowercase, the spelling `GET /channels` reports) and the
# Solana key (base58).
evm_settlement_address() {
  cast wallet address --private-key "0x$(cat "$(node_dir "$1")/settlement.key")" | tr 'A-F' 'a-f'
}
solana_settlement_address() {
  base58 "$("$CONNECTOR" send --operator-key "$(node_dir "$1")/settlement-solana.key" \
    --print-keyid)"
}

# Which SPL mint this topology's Solana channels settle in. For every topology
# but `dealing` it is the shared mock USDC mint `make solana-mint-usdc` seeds.
# For a topology that owns a mint, the keypair is derived (not generated, not
# committed) and the address it implies is asserted against every committed
# config that settles on Solana, which is what makes committing it legitimate.
resolve_solana_mint() {
  if [[ -z "$TOPOLOGY_MINT_INDEX" ]]; then
    SOLANA_MINT="$SOLANA_USDC_MINT"
    return
  fi

  derived_key "$TOPOLOGY_MINT_INDEX" >"$KEYS/mint.key"
  chmod 600 "$KEYS/mint.key"
  solana_cli_keypair "$KEYS/mint.key" "$KEYS/mint.json"
  SOLANA_MINT="$(solana address --keypair "$KEYS/mint.json")"

  local entry node config
  for entry in $NODES; do
    node="${entry%%:*}"
    config="$(node_config "$node")"
    grep -q '^\[settlement.solana\]' "$config" || continue
    config_must_name "$SOLANA_MINT" "$config" \
      "this topology's own ${TOPOLOGY_MINT_DECIMALS}-decimal mock mint"
  done
}

# ── Talking to a running node's operator surface ─────────────────────────────
# Every write is RFC 9421-signed with the node's own allowlisted
# `operator-send.key` by `docs/operators/sign-write.sh` -- the shipped signer,
# whose output `crates/connector-cli/tests/sign_write_script.rs` holds to the
# node's own verifier -- and every read carries the node's bearer token.
local_compose() {
  docker compose -f "$ROOT/docker-compose.yml" -f "$HERE/$TOPOLOGY/compose.yml" \
    --profile evm --profile solana --profile "$TOPOLOGY" "$@"
}

# `operator_request <node> <METHOD> <path> [body]`: the answer's body on
# stdout, or a non-zero exit naming the node, the request and the answer.
#
# A node with a host port is reached from here with curl. One without --
# onion's B -- is reached from INSIDE its own container with the image's own
# busybox wget, over its loopback: the node has no other door, and publishing
# one for this script would give it one.
operator_request() {
  local node="$1" method="$2" path="$3" body="${4:-}"
  local dir port
  dir="$(node_dir "$node")"
  port="$(node_port "$node")"

  local -a headers=()
  if [[ "$method" == "GET" ]]; then
    headers=("Authorization: Bearer $(cat "$dir/operator-bearer-token")")
  else
    local line
    while IFS= read -r line; do
      headers+=("$line")
    done < <(bash "$SIGN_WRITE" -k "$dir/operator-send.key" -X "$method" -p "$path" -b "$body")
  fi

  local answer status
  if [[ "$port" != "-" ]]; then
    local -a args=(-sS -X "$method" -o /dev/stdout -w '\n%{http_code}')
    local header
    for header in "${headers[@]}"; do
      args+=(-H "$header")
    done
    if [[ "$method" != "GET" ]]; then
      args+=(-H "Content-Type: application/json" --data-binary "$body")
    fi
    answer="$(curl "${args[@]}" "http://127.0.0.1:$port$path")" || {
      echo "ERROR: $node did not answer $method $path on 127.0.0.1:$port." >&2
      exit 1
    }
    status="${answer##*$'\n'}"
    answer="${answer%$'\n'*}"
  else
    local -a args=(-q -O -)
    local header
    for header in "${headers[@]}"; do
      args+=(--header "$header")
    done
    if [[ "$method" != "GET" ]]; then
      args+=(--header "Content-Type: application/json" --post-data "$body")
    fi
    # busybox wget exits non-zero on any non-2xx answer and prints no body
    # for it, so its exit status IS the verdict; its complaint goes to its own
    # file rather than into the answer the caller parses.
    local wget_errors
    wget_errors="$(mktemp)"
    if answer="$(local_compose exec -T "$node" wget "${args[@]}" \
      "http://127.0.0.1:3000$path" 2>"$wget_errors")"; then
      status=2xx
      rm -f "$wget_errors"
    else
      status="non-2xx (busybox wget: $(tr '\n' ' ' <"$wget_errors"))"
      rm -f "$wget_errors"
      answer=""
    fi
  fi

  if [[ ! "$status" =~ ^2([0-9][0-9]|xx)$ ]]; then
    echo "ERROR: $node answered $method $path with $status" >&2
    [[ -n "$body" ]] && echo "       request: $body" >&2
    [[ -n "$answer" ]] && echo "       answer:  $answer" >&2
    echo "       'docker compose logs $node' has the node's side of it." >&2
    exit 1
  fi
  printf '%s' "$answer"
}

# The URL a node's self-description is served at, as the OTHER containers of
# its topology reach it -- which is what `POST /peers` is given.
node_url() {
  if [[ "$TOPOLOGY" == "onion" ]]; then
    echo "http://$(cat "$(node_dir "$1")/onion-hostname")/ilp"
  else
    echo "http://$1:3000/ilp"
  fi
}

# ── The onion topology's one extra step ──────────────────────────────────────
# A hidden-service address does not exist until its daemon has run. `anon`
# generates it into `HiddenServiceDir/hostname` the first time it starts, and
# ADR 0070 decision 7 says what happens next: THE OPERATOR COPIES IT DOWN, into
# `[node]` and wherever a peer names it. The connector never reads that file
# and never speaks the daemon's control protocol.
#
# This function is that operator step, automated exactly as far as it goes: it
# starts the sidecars, reads BOTH addresses -- each node has one, because each
# end of the peering reads the other's self-description -- and writes the
# committed configs out again with each placeholder substituted.
onion_address() {
  local sidecar="$1" address="" attempt
  for attempt in $(seq 1 60); do
    address="$(local_compose exec -T "$sidecar" \
      cat /var/lib/anon/hidden_service/hostname 2>/dev/null | tr -d '\r\n' || true)"
    if [[ -n "$address" ]]; then
      break
    fi
    sleep 2
  done
  # `.anyone` and not `.onion`: an address ending in `.onion` means the sidecar
  # came from somewhere else -- a stale `anon-live:v0.4.10.2` tag, or a compose
  # file edited to pull the old published image (issue #1284).
  if [[ ! "$address" =~ ^[a-z2-7]{56}\.anyone$ ]]; then
    echo "ERROR: $sidecar has not produced a v3 .anyone address." >&2
    echo "       Read back: '${address:-<nothing>}'" >&2
    echo "       The daemon writes it into /var/lib/anon/hidden_service/hostname on its first" >&2
    echo "       start. If the container is not running, the usual cause is the terms flag:" >&2
    echo "       'anon' exits rather than prompting when 'AgreeToTerms 1' is missing from its" >&2
    echo "       anonrc. 'docker compose logs $sidecar' says which." >&2
    if [[ "$address" == *.onion ]]; then
      echo "       An address ending in .onion means the sidecar is anon v0.4.9.7, from before" >&2
      echo "       upstream renamed the TLD. 'docker image rm anon-live:v0.4.10.2' clears it." >&2
    fi
    exit 1
  fi
  echo "$address"
}

render_onion_configs() {
  need docker "The onion topology's addresses are generated by containers; nothing else has them."

  echo "onion: starting the anon sidecars -- the addresses do not exist until the daemons have run"
  local_compose up -d anon-a anon-b

  # The hostname file appears at KEY GENERATION, seconds after start and long
  # before the daemon has bootstrapped -- which is why this waits for the file
  # rather than for the healthcheck.
  local address_a address_b
  address_a="$(onion_address anon-a)"
  address_b="$(onion_address anon-b)"
  echo "onion: connector-a is reachable at $address_a, connector-b at $address_b"
  echo "$address_a" >"$(node_dir connector-a)/onion-hostname"
  echo "$address_b" >"$(node_dir connector-b)/onion-hostname"

  local entry node config rendered placeholder
  for entry in $NODES; do
    node="${entry%%:*}"
    config="$(node_config "$node")"
    rendered="$(node_dir "$node")/connector.toml"
    for placeholder in "$ONION_PLACEHOLDER_A" "$ONION_PLACEHOLDER_B"; do
      if [[ "$node" == connector-a && "$placeholder" == "$ONION_PLACEHOLDER_A" ]] ||
        [[ "$node" == connector-b && "$placeholder" == "$ONION_PLACEHOLDER_B" ]]; then
        if ! grep -qF "$placeholder" "$config"; then
          echo "ERROR: $config names no '$placeholder' host, so there is nothing to substitute" >&2
          echo "       its own daemon's address into -- rendering it anyway would mount a config" >&2
          echo "       publishing a host no circuit reaches." >&2
          exit 1
        fi
      fi
    done
    {
      echo "# GENERATED by local/keys.sh from local/$TOPOLOGY/$node.toml -- do not edit."
      echo "# Each hidden-service host below was read out of its own daemon's"
      echo "# HiddenServiceDir/hostname and written in here, which is ADR 0070 decision 7:"
      echo "# the daemon generates the address and the OPERATOR copies it down."
      sed -e "s/$ONION_PLACEHOLDER_A/$address_a/g" -e "s/$ONION_PLACEHOLDER_B/$address_b/g" "$config"
    } >"$rendered"
    chmod a+r "$rendered"
    if grep -qF placeholderplaceholder "$rendered"; then
      echo "ERROR: $rendered still names a placeholder after substitution." >&2
      exit 1
    fi
    echo "$node: rendered $rendered"
  done
  chmod a+r "$(node_dir connector-a)/onion-hostname" "$(node_dir connector-b)/onion-hostname"
}

# ── Stage two: the peerings ──────────────────────────────────────────────────

# The channel `$2` of node `$1`, as its own `GET /channels` reports it: one
# JSON object, or nothing.
channel_view() {
  operator_request "$1" GET /channels | python3 -c '
import json, sys
wanted = sys.argv[1]
for row in json.load(sys.stdin):
    if row.get("id") == wanted and row.get("scheme") == "batch-settlement":
        print(json.dumps(row))
        break
' "$2"
}

# Read an EVM channel off `x402BatchSettlement` itself and require at least
# `$2` base units in it. The chain's own `channels(id)` -- never the node's
# answer -- because the point is to catch a node that SAYS it funded a channel.
assert_evm_channel() {
  local id="$1" target="$2" label="$3" balance
  balance="$(cast call "$X402_BATCH_SETTLEMENT" "channels(bytes32)(uint128,uint128)" "$id" \
    --rpc-url "$ANVIL_RPC" | head -1 | cut -d' ' -f1)"
  if [[ -z "$balance" || "$balance" -lt "$target" ]]; then
    echo "ERROR: '$label': x402BatchSettlement holds ${balance:-nothing} in channel $id, not the" >&2
    echo "       $target its payer was topped up to." >&2
    exit 1
  fi
  echo "'$label': x402BatchSettlement holds $balance in $id (at least $target)"
}

# Read a Solana channel off the validator and require the program's own layout
# to agree with the peering: owned by `payment-channels`, Open, paid by `$2`,
# payable to `$3`, signed for by `$2` (ADR 0075 decision 3: the payer's
# settlement key is its `authorized_signer`), in this topology's mint, holding
# at least `$4`. Offsets are `connector_settlement_solana::batch::wire`'s.
assert_solana_channel() {
  local id="$1" payer="$2" payee="$3" target="$4" label="$5"
  solana account "$id" --output json --commitment confirmed --url "$SOLANA_RPC" |
    python3 -c '
import base64, json, sys
alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"

def b58(raw):
    value = int.from_bytes(raw, "big")
    out = ""
    while value:
        value, rem = divmod(value, 58)
        out = alphabet[rem] + out
    return alphabet[0] * (len(raw) - len(raw.lstrip(b"\0"))) + out

program, payer, payee, mint, target, label, channel = sys.argv[1:8]
account = json.load(sys.stdin)["account"]
data = base64.b64decode(account["data"][0])
owner = account["owner"]
deposit = 0
problems = []
if owner != program:
    problems.append(f"owned by {owner}, not {program}")
if len(data) != 256:
    problems.append(f"{len(data)} bytes, not the 256 of a payment-channels Channel")
else:
    if data[3] != 0:
        problems.append(f"status byte {data[3]}, not Open")
    deposit = int.from_bytes(data[12:20], "little")
    for name, offset, want in (("payer", 88, payer), ("payee", 120, payee),
                               ("authorized_signer", 152, payer), ("mint", 184, mint)):
        got = b58(data[offset:offset + 32])
        if got != want:
            problems.append(f"{name} is {got}, not {want}")
    if deposit < int(target):
        problems.append(f"deposit is {deposit}, not at least {target}")
if problems:
    print(f"ERROR: {label}: the channel account disagrees with the peering:", file=sys.stderr)
    for problem in problems:
        print(f"         {problem}", file=sys.stderr)
    sys.exit(1)
print(f"{label}: payment-channels holds {deposit} in {channel} (at least {target}), paid by {payer} to {payee}")
' "$PAYMENT_CHANNELS_PROGRAM" "$payer" "$payee" "$SOLANA_MINT" "$target" "'$label'" "$id"
}

# `POST /peers` from `$1` naming `$2`, on `$3`, at fee `$4` and cap `$5`,
# opening with OPEN_DEPOSIT: the established channel's id on stdout.
establish() {
  local from="$1" to="$2" chain="$3" fee="$4" cap="$5" id="$6"
  local body answer channel status
  body="$(printf '{"id":"%s","url":"%s","fee":%s,"max_packet_amount":%s,"chain":"%s","deposit":%s}' \
    "$id" "$(node_url "$to")" "$fee" "$cap" "$chain" "$OPEN_DEPOSIT")"
  # On `onion` the other node's self-description is read over a circuit, and a
  # freshly published hidden service takes a while to become reachable -- the
  # rehearsal waits up to five minutes for its rendezvous for the same reason.
  # A write that fails there is retried on the same budget; everywhere else a
  # failure is a failure the first time.
  local attempts=1 attempt
  [[ "$TOPOLOGY" == "onion" ]] && attempts=20
  for attempt in $(seq 1 "$attempts"); do
    if answer="$(operator_request "$from" POST /peers "$body")"; then
      break
    fi
    if ((attempt == attempts)); then
      exit 1
    fi
    echo "'$id': $from could not reach $to's self-description yet (attempt $attempt); retrying" >&2
    sleep 15
  done
  channel="$(json_field 'doc["channel"]["id"]' <<<"$answer")"
  status="$(json_field 'doc["channel"]["status"]' <<<"$answer")"
  if [[ -z "$channel" || "$(json_field 'doc["channel"]["chain"]' <<<"$answer")" != "$chain" ]]; then
    echo "ERROR: $from's POST /peers for '$id' answered without a $chain channel: $answer" >&2
    exit 1
  fi
  echo "'$id': $from -> $to on $chain, channel $channel ($status)" >&2
  echo "$channel"
}

establish_peerings() {
  if [[ -z "$PEERINGS" ]]; then
    echo "'$TOPOLOGY' has no peering; nothing to establish."
    return
  fi
  mkdir -p "$KEYS/peerings"

  local peering id chain payer payee fee cap
  for peering in $PEERINGS; do
    IFS=':' read -r id chain payer payee fee cap <<<"$peering"

    # The PAYEE first: its write reads the payer's self-description and binds
    # the payer's voucher signer to the peering, so every voucher the payer
    # then sends arrives in the PEER role. Its own channel toward the payer is
    # the other half of the peering, opened and never paid on here.
    establish "$payee" "$payer" "$chain" 0 0 "$id" >/dev/null
    local channel
    channel="$(establish "$payer" "$payee" "$chain" "$fee" "$cap" "$id")"

    local target counterparty
    if [[ "$chain" == "evm" ]]; then
      target="$CHANNEL_DEPOSIT"
      counterparty="$(evm_settlement_address "$payee")"
    else
      target="$SOLANA_CHANNEL_DEPOSIT"
      counterparty="$(solana_settlement_address "$payee")"
    fi

    # The payer's own view of the channel, to find the shortfall -- and to
    # check it is the channel this peering should be paying on: outbound, to
    # the payee's settlement identity.
    local view collateral
    view="$(channel_view "$payer" "$channel")"
    if [[ -z "$view" ]]; then
      echo "ERROR: $payer's GET /channels does not list the channel its POST /peers answered: $channel" >&2
      exit 1
    fi
    if [[ "$(json_field 'doc["direction"]' <<<"$view")" != "outbound" ||
      "$(json_field 'doc["counterparty"]' <<<"$view" | tr 'A-F' 'a-f')" != "$(echo "$counterparty" | tr 'A-F' 'a-f')" ]]; then
      echo "ERROR: '$id': $payer's channel $channel is not outbound to $payee ($counterparty): $view" >&2
      exit 1
    fi
    # Read off the chain by the node; absent only while the open is still in
    # flight or the chain could not be read -- either way nothing to fund
    # against, so it is a failure rather than a zero.
    collateral="$(json_field 'doc["collateral"]' <<<"$view")"
    if [[ ! "$collateral" =~ ^[0-9]+$ ]]; then
      echo "ERROR: '$id': $payer reports no collateral for $channel: $view" >&2
      exit 1
    fi
    if ((collateral < target)); then
      local shortfall=$((target - collateral))
      operator_request "$payer" POST "/channels/$channel/fund" "{\"amount\":$shortfall}" >/dev/null
      echo "'$id': $payer topped $channel up by $shortfall to $target"
    else
      echo "'$id': $channel already holds $collateral; nothing to top up"
    fi

    if [[ "$chain" == "evm" ]]; then
      assert_evm_channel "$channel" "$target" "$id"
    else
      assert_solana_channel "$channel" "$(solana_settlement_address "$payer")" "$counterparty" \
        "$target" "$id"
    fi

    # The payee journals every voucher it accepts under this key
    # (`client-edge-claims.log`): what the rehearsal greps for.
    echo "$chain:$channel" >"$KEYS/peerings/$id"
  done

  local route node prefix peer_id price
  for route in $ROUTES; do
    IFS=':' read -r node prefix peer_id price <<<"$route"
    operator_request "$node" POST /routes/peers \
      "{\"prefix\":\"$prefix\",\"peer_id\":\"$peer_id\",\"price\":$price}" >/dev/null
    echo "$node: routes $prefix to '$peer_id' at price $price"
  done

  chmod -R a+rX "$KEYS/peerings"
}

if [[ "$STAGE" == "channels" ]]; then
  if [[ ! -d "$KEYS" ]]; then
    echo "ERROR: $KEYS does not exist. Run 'local/keys.sh $TOPOLOGY' first -- this stage" >&2
    echo "       signs operator writes with keys that stage provisions." >&2
    exit 1
  fi
  need docker "The channels stage reaches a node with no host port through its own container."
  resolve_solana_mint
  establish_peerings
  exit 0
fi

mkdir -p "$KEYS"
chmod 700 "$KEYS"
# A previous run's channel ids name channels on a chain that has since been
# reset; the `channels` stage writes this run's.
rm -rf "$KEYS/peerings"

resolve_solana_mint

# ── Keys, per node ───────────────────────────────────────────────────────────
for entry in $NODES; do
  IFS=':' read -r node evm_index solana_index _port <<<"$entry"
  dir="$(node_dir "$node")"
  config="$(node_config "$node")"

  if [[ ! -f "$config" ]]; then
    echo "ERROR: $config does not exist, but this script's topology table lists node" >&2
    echo "       '$node' for '$TOPOLOGY'. A node's key directory is named after its" >&2
    echo "       compose service and its config file after the same name." >&2
    exit 1
  fi

  mkdir -p "$dir"
  chmod 700 "$dir"

  # 64 hex characters each -- one of the two shapes every `key_file` in this
  # repository accepts. Random, and kept across runs.
  for key in signer operator-send; do
    if [[ ! -f "$dir/$key.key" ]]; then
      openssl rand -hex 32 >"$dir/$key.key"
      chmod 600 "$dir/$key.key"
      echo "$node: generated $key.key"
    fi
  done

  # The operator surface's READ credential. A token, not a key: it gates reads
  # and nothing else, and no shared secret can move value (ADR 0008).
  if [[ ! -f "$dir/operator-bearer-token" ]]; then
    openssl rand -hex 32 >"$dir/operator-bearer-token"
    chmod 600 "$dir/operator-bearer-token"
    echo "$node: generated operator-bearer-token"
  fi

  # The two DERIVED keys, rewritten every run: a pure function of the mnemonic
  # and the index.
  derived_key "$evm_index" >"$dir/settlement.key"
  derived_key "$solana_index" >"$dir/settlement-solana.key"
  chmod 600 "$dir/settlement.key" "$dir/settlement-solana.key"

  # The Solana CLI's own keypair file, built from the derived seed -- so the
  # account this script airdrops to is provably the account the connector
  # signs as, and `solana-keygen verify` is a real signature round trip.
  solana_keyid="$("$CONNECTOR" send --operator-key "$dir/settlement-solana.key" --print-keyid)"
  solana_cli_keypair "$dir/settlement-solana.key" "$dir/settlement-solana-cli.json"
  solana_address="$(base58 "$solana_keyid")"
  solana-keygen verify "$solana_address" "$dir/settlement-solana-cli.json" >/dev/null

  # The write allowlist: the PUBLIC half of operator-send.key, derived by the
  # binary that will verify it, so the allowlisted value and the signature
  # cannot disagree.
  keyid="$("$CONNECTOR" send --operator-key "$dir/operator-send.key" --print-keyid)"
  {
    echo "# Written by local/keys.sh -- the public half of operator-send.key."
    echo "# An allowlist entry is an ed25519 PUBLIC key and holds no secret."
    echo "$keyid"
  } >"$dir/operator-write-keys"

  # The body the sender posts. A fixture; it lives beside the keys so the
  # sender container mounts exactly one directory.
  if [[ ! -f "$dir/payload.json" ]]; then
    echo '{"hello":"from a paid packet"}' >"$dir/payload.json"
  fi

  echo "$node: evm $(evm_settlement_address "$node")  solana $solana_address  operator keyid $keyid"
done

# The connector containers mount these directories READ-ONLY as uid 10001, so
# the files must be world-readable to them. They are mode 600 above for the
# host's sake; relax now that generation is done. These are disposable
# local-chain keys and none of them has ever held value anywhere else.
chmod -R a+rX "$KEYS"

# ── Funding ──────────────────────────────────────────────────────────────────
# The USDC must actually be ON the chain before anything mints from it: a
# `cast send` of `mint(...)` to an address with no code does NOT revert -- it
# is an ordinary call to a plain account -- so without this check a race
# against the seed reports a funded account and leaves an empty one. The
# compose anvil healthcheck gates on the same fact; this is the second half,
# for anyone running the script against a chain they brought up themselves.
if [[ ! "$(cast code "$USDC" --rpc-url "$ANVIL_RPC" 2>/dev/null)" =~ ^0x[0-9a-fA-F]+$ ]]; then
  echo "ERROR: no contract at $USDC on $ANVIL_RPC." >&2
  echo "       The local chain is not seeded yet -- infra/anvil/seed.sh runs as part of the" >&2
  echo "       compose anvil service's startup. Wait for that container to report healthy" >&2
  echo "       (it gates on exactly this) and re-run." >&2
  exit 1
fi

# A throwaway `solana` config pointed at the mock mint's authority, so that
# authority is the default signer AND fee payer of every `spl-token` call
# below. It never touches the developer's own `~/.config/solana`.
SOLANA_SPL_CONFIG="$(mktemp)"
trap 'rm -f "$SOLANA_SPL_CONFIG"' EXIT
solana -C "$SOLANA_SPL_CONFIG" config set \
  --keypair "$SOLANA_USDC_AUTHORITY" --url "$SOLANA_RPC" >/dev/null

# A topology that owns its own mint creates it HERE, under that same authority.
# Idempotent: the mint is left alone if the validator already has it, and the
# treasury is topped up either way.
if [[ -n "$TOPOLOGY_MINT_INDEX" ]]; then
  solana -C "$SOLANA_SPL_CONFIG" airdrop 100 \
    "$(solana-keygen pubkey "$SOLANA_USDC_AUTHORITY")" >/dev/null 2>&1 || true
  if solana -C "$SOLANA_SPL_CONFIG" account "$SOLANA_MINT" >/dev/null 2>&1; then
    echo "'$TOPOLOGY': mint $SOLANA_MINT already exists ($TOPOLOGY_MINT_DECIMALS decimals)"
  else
    echo "'$TOPOLOGY': creating mint $SOLANA_MINT ($TOPOLOGY_MINT_DECIMALS decimals)"
    spl-token create-token --config "$SOLANA_SPL_CONFIG" \
      --decimals "$TOPOLOGY_MINT_DECIMALS" "$KEYS/mint.json" >/dev/null
  fi
  spl-token create-account --config "$SOLANA_SPL_CONFIG" "$SOLANA_MINT" >/dev/null 2>&1 || true
  spl-token mint --config "$SOLANA_SPL_CONFIG" "$SOLANA_MINT" "$TOPOLOGY_MINT_TREASURY" >/dev/null
fi

# The treasury has to actually hold the supply before anything is handed out;
# `spl-token transfer` against a mint that does not exist reports "Account not
# found", which names neither the mint nor the step that was skipped.
if ! treasury="$(spl-token balance --config "$SOLANA_SPL_CONFIG" "$SOLANA_MINT" 2>&1)"; then
  echo "ERROR: the mock treasury for $SOLANA_MINT on $SOLANA_RPC holds nothing spendable." >&2
  echo "       spl-token said: $treasury" >&2
  if [[ -n "$TOPOLOGY_MINT_INDEX" ]]; then
    echo "       That mint belongs to the '$TOPOLOGY' topology and is created a few lines above" >&2
    echo "       this check, under the authority infra/solana/usdc-authority.json; most likely" >&2
    echo "       the authority has no SOL on a validator 'make solana-mint-usdc' never ran against." >&2
  else
    echo "       Mint $SOLANA_MINT is created and seeded by infra/solana/create-usdc-mint.sh --" >&2
    echo "       run 'make solana-mint-usdc' against a running validator and re-run this script." >&2
  fi
  exit 1
fi
echo "the $SOLANA_MINT treasury holds $treasury; $NODE_SOLANA_TOKENS goes to each node"

for entry in $NODES; do
  node="${entry%%:*}"
  dir="$(node_dir "$node")"

  evm_address="$(evm_settlement_address "$node")"
  echo "$node: funding EVM settlement account $evm_address"
  cast send --rpc-url "$ANVIL_RPC" --private-key "$ANVIL_ACCOUNT0_KEY" \
    --value 100ether "$evm_address" >/dev/null
  # FiatToken v2.2 is MINTABLE by its configured minter -- anvil's account 1,
  # with an unlimited allowance (`infra/anvil/seed.sh`) -- so this is a mint
  # rather than a transfer out of somebody's balance.
  cast send --rpc-url "$ANVIL_RPC" --private-key "$USDC_MINTER_KEY" \
    "$USDC" "mint(address,uint256)" "$evm_address" "$NODE_EVM_USDC" >/dev/null
  echo "  100 ETH + $NODE_EVM_USDC USDC base units (6dp), minted"

  solana_address="$(solana address --keypair "$dir/settlement-solana-cli.json")"
  echo "$node: funding Solana settlement account $solana_address"
  solana airdrop 100 "$solana_address" --url "$SOLANA_RPC" >/dev/null
  # SOL pays fees; it is not the asset a channel settles in. A TRANSFER, not a
  # mint: an SPL mint has one authority and the supply already sits in its
  # treasury. `--fund-recipient` because the associated token account does not
  # exist until something creates it, and AFTER the airdrop because
  # `--fund-recipient` refuses a recipient wallet holding no SOL.
  spl-token transfer --config "$SOLANA_SPL_CONFIG" --fund-recipient \
    "$SOLANA_MINT" "$NODE_SOLANA_TOKENS" "$solana_address" >/dev/null
  echo "  100 SOL + $NODE_SOLANA_TOKENS of $SOLANA_MINT (${TOPOLOGY_MINT_DECIMALS}dp)"
done

# ── The addresses the daemons generate, copied where the operator would ──────
# Last, because it is the only step that needs a container running rather than
# a chain. `make local-up` mounts the RENDERED files; the committed ones are
# what gets reviewed.
if [[ "$TOPOLOGY" == "onion" ]]; then
  render_onion_configs
fi

echo
echo "keys for '$TOPOLOGY' are in $KEYS (gitignored)"
