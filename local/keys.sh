#!/usr/bin/env bash
# =============================================================================
# Provision the key material a local topology needs, and fund it.
#
#   local/keys.sh <topology>                  # e.g. local/keys.sh solo
#   local/keys.sh <topology> solana-channels  # after the nodes are serving
#
# Everything lands in local/.keys/<topology>/<node>/, which is GITIGNORED.
# Nothing this script writes is ever committed: ADR 0012 makes key material a
# location rather than a value, and every committed connector.toml under
# local/ names these as paths.
#
# One directory per NODE, named after that node's compose service, and the
# node's config file is `local/<topology>/<node>.toml`. A topology with one
# node (`solo`) is not a special case of that -- it is the same rule with one
# entry -- which is what let two more topologies land here without a second
# provisioning path.
#
# This replaces `deploy/connector-rust/local-stack/prepare.sh`, deleted with
# that bundle. What it adds is the FUNDING: a key that exists is not a key that
# can pay gas, and "the connector refused to start" and "its settlement account
# has no ETH" are different problems that used to present identically.
#
# No faucet is involved on either chain. The faucet is an app-layer service and
# is not part of the connector; local chains fund from genesis.
#
# Idempotent. Re-running keeps existing keys, re-funds them, and leaves an
# already-open payment channel alone -- which is the common case: both local
# chains wipe their state on every start, so the accounts survive in this
# directory while their balances do not.
#
# Idempotent about the channel COLLATERAL too, and that one took work rather
# than coming for free. The EVM leg's `setTotalDeposit` takes an absolute
# total; the operator surface's `POST /channels/:id/fund` takes an increment,
# so the Solana leg reads the payer's own on-chain deposit first and tops up
# the shortfall. A second `make local-up` deposits nothing, which is the
# difference between a setup script and a setup script that quietly doubles
# a number every time somebody runs it.
#
# ── Why some keys are RANDOM and some are DERIVED ────────────────────────────
#
# A multi-node topology has to write one node's address into another node's
# committed config: a `[[peer_channels]]` row names the `counterparty_key`
# whose signature this node accepts on a claim, and there is no way to say
# "whatever address the other container happens to have generated". So:
#
#   * `signer.key` and `operator-send.key` are RANDOM. Neither ever appears in
#     a committed file -- the signer is an identity the packet discovers over
#     `GET /ilp/identity`, and the operator allowlist holds only the PUBLIC
#     half, written here at run time.
#
#   * `settlement.key` and `settlement-solana.key` are DERIVED, per node, from
#     anvil's own published test mnemonic at a fixed index. Their ADDRESSES
#     are what the committed `[[peer_channels]]` rows name, so they have to be
#     the same on every machine and after every reset.
#
# The mnemonic is public knowledge -- anvil prints it on every start and this
# repository already commits account 0's private key below, for the same
# reason: it is the deployer of a disposable local chain. Deriving from it
# introduces no secret that did not already exist, and the alternative (a
# fixed "throwaway" key checked in under local/) would introduce one.
#
# EVM and Solana take DISJOINT index ranges, so no single 32 bytes is ever
# used on both curves. The Solana derivation is not BIP44-for-Solana and does
# not claim to be: `[settlement.solana.key]` is a raw 32-byte SEED that the
# connector hands to `keypair_from_seed`, and a BIP32 derivation is just a
# deterministic 32-byte function of (mnemonic, index), so it supplies one.
# =============================================================================
set -euo pipefail

TOPOLOGY="${1:-}"
# Two stages, because they cannot run at the same moment. `keys` is everything
# that has to exist BEFORE a node starts -- the key files it mounts, their
# funding on both chains, and the EVM peering channels. `solana-channels` is
# the pair of things that can only happen AFTER: a Solana channel is opened by
# submitting an `InitializeChannel` and collateralised by submitting a
# `Deposit` signed by the depositing participant, and the only thing in this
# repository that can submit either is a RUNNING connector's `POST /channels`
# and `POST /channels/:id/fund` (`local/open-solana-channel.py` has the long
# version). `make local-up` calls both stages, in that order, with the
# connectors started in between.
STAGE="${2:-keys}"
if [[ -z "$TOPOLOGY" ]]; then
  echo "usage: local/keys.sh <topology> [stage]" >&2
  echo "       topology: solo, two-hop, mixed-chain, onion, dealing" >&2
  echo "       stage:    keys (default, pre-boot) | solana-channels (post-boot)" >&2
  exit 1
fi
if [[ "$STAGE" != "keys" && "$STAGE" != "solana-channels" ]]; then
  echo "ERROR: unknown stage '$STAGE'. Known stages: keys, solana-channels." >&2
  exit 1
fi

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
KEYS="$HERE/.keys/$TOPOLOGY"
CONNECTOR="$ROOT/target/release/connector"

ANVIL_RPC="${ANVIL_RPC:-http://127.0.0.1:8545}"
SOLANA_RPC="${SOLANA_RPC:-http://127.0.0.1:8899}"

# anvil's own published account 0 and the mnemonic it derives from -- "test
# test ... junk", public knowledge, printed by anvil on every start. Account 0
# is the deployer `DeployLocal.s.sol` runs as, so it owns the settlement
# topology and can mint the mock USDC. Only ever pointed at a disposable local
# chain.
ANVIL_MNEMONIC="test test test test test test test test test test test junk"
ANVIL_ACCOUNT0_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
MOCK_USDC=0x5FbDB2315678afecb367f032d93F642f64180aa3
TOKEN_NETWORK_REGISTRY=0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512

# The deterministic mock mint and the payment-channel program id the local
# validator loads at genesis (`infra/solana/entrypoint.sh`,
# `infra/solana/create-usdc-mint.sh`). Needed here only to derive a Solana
# peering's channel PDA, which is a pure function of the program, the mint and
# the two participants.
SOLANA_USDC_MINT=H8HSreUF2s8r8hem4qMttE3bWYCpFuh71jbuos5bA77H
SOLANA_PROGRAM_ID=HY4AYFNe5Vg5BkEwAURNsGY3uFAvGMNpAQPRtgoasJiR

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
# USDC-on-the-validator are two `AssetId`s, so a dealing node is held to the
# absence rule for that pair like any other -- but they are the same asset at
# the same scale, and a crossing at par cannot tell a correct conversion from
# no conversion at all. A thousandfold scale difference can, so that leg
# settles in a NINE-decimal mock instead, created below.
#
# Its KEYPAIR is derived, at a fixed index of the same public anvil mnemonic
# every settlement key here comes from, for the same reason: the ADDRESS is
# committed in `local/dealing/connector-{b,c}.toml` and has to be identical on
# every machine and after every `--reset`, while nothing secret may be written
# down. Its AUTHORITY is the committed `usdc-authority.json` above -- one
# allowlisted local-chain key, not a second one.
#
# Deliberately NOT in `infra/solana/create-usdc-mint.sh`: that script seeds the
# mint every other config in this repository names, devnet included, and this
# token exists only for one local topology.
DEALING_MINT_INDEX=28
DEALING_MINT_DECIMALS=9
# A UI amount, like every other `spl-token` figure -- the same 100M that script
# mints itself, so neither treasury is ever the reason something fails.
DEALING_MINT_TREASURY=100000000

# What a peering's payer deposits into its channel, in 6-decimal USDC base
# units: 100 USDC against a 1000 µUSDC crossing, so a topology can be
# rehearsed a hundred thousand times before the collateral is the reason
# something fails. One figure for both chains.
#
# The two legs reach it differently, and the difference is the whole of the
# idempotence question here. `setTotalDeposit` takes a TOTAL, so re-running the
# EVM leg re-asserts the figure. `POST /channels/:id/fund` takes an INCREMENT
# (`FundChannelRequest`), so the Solana leg would add another 100 USDC on every
# run -- `open-solana-channel.py` reads the payer's own on-chain deposit first
# and deposits only the shortfall, which is what makes the two behave alike.
CHANNEL_DEPOSIT=100000000

# The same 100 tokens on a leg that settles in the 9-decimal mock rather than
# in 6-decimal USDC -- a thousand times the base units, because base units are
# what a deposit is denominated in. ADR 0071 seen from the collateral side: a
# figure copied across a denomination boundary unconverted is wrong by exactly
# the scale gap, and the EVM leg of that same topology still deposits
# CHANNEL_DEPOSIT because its channel holds USDC.
DEALING_CHANNEL_DEPOSIT=100000000000

# What each node's Solana settlement account is given of the topology's own
# mint, as a UI amount (`spl-token` takes UI amounts, not base units) -- 1000
# tokens, the same figure the EVM leg mints as 1000000000 at 6 decimal places.
# A UI amount is scale-free, which is why this one figure serves a 6-decimal
# and a 9-decimal mint alike; ten times CHANNEL_DEPOSIT either way, so a node
# can collateralise its channel and still be visibly holding tokens afterwards.
NODE_USDC=1000

# ── The onion topology's placeholder address ─────────────────────────────────
# `local/onion/*.toml` are committed with this host wherever the daemon's real
# address goes, and this script substitutes that address for it (see
# `render_onion_configs`). It is 56 characters of the base32 alphabet a v3
# onion address is drawn from, and it ends in `.anyone`, so the committed
# files load through the real parser -- which is the whole reason they are
# committed rather than generated.
#
# `.anyone`, not `.onion`, because that is the TLD the daemon `local/anon-image`
# pins writes: `anon` renamed it and the rename is total, so a placeholder
# spelled the old way would advertise a TLD nothing in this topology generates
# (issue #1284). The connector itself takes either spelling.
#
# `local_topologies_load.rs` holds this literal and both committed files to one
# string: a placeholder renamed here and not there renders configs that still
# name a host no circuit reaches, and the failure would arrive as a `T01` two
# steps later.
ONION_PLACEHOLDER=placeholderplaceholderplaceholderplaceholderplaceholderb.anyone

# ── The topology table ───────────────────────────────────────────────────────
# `node:evm_index:solana_index` per node, and `id:chain:payer:payee` per
# peering. The payer is the side that OPENS and FUNDS the channel, and it is
# the side packets flow away from: debt flows in the direction packets do
# (`peer-semantics-pre-868.md` §6.4), so the payer is the side that can
# actually be owed against.
#
# A SOLANA peering carries a fifth field: the host port that compose publishes
# the payer's client edge on. Its channel is opened through that node's own
# operator surface rather than with a chain CLI, so this table has to know
# where to reach it. An EVM peering has no fifth field and needs none -- `cast`
# talks to the chain, not to a node.
#
# Indices are disjoint across topologies as well as across nodes. They need
# not be -- no two topologies are ever up at once -- but a settlement address
# that appears in exactly one committed config is one fewer thing to
# disambiguate when a claim is refused and the log names an address.
#
# EVM and Solana must also never share an index, so that no 32 bytes is ever
# used on both curves. The first four topologies took 4-11 for EVM and 14-21
# for Solana; `dealing` needs three more of each and there are only two EVM
# slots left below 14, so it takes a fresh pair of blocks above both rather
# than reaching into the Solana one. What the rule asks for is disjointness,
# not contiguity.
#
# A topology may also own a MINT -- `TOPOLOGY_MINT_INDEX`, a third derivation
# from the same mnemonic, for a topology that settles in a token the shared
# mock USDC mint is not. Empty for every topology that settles in that mint,
# which is all of them but `dealing`.
TOPOLOGY_MINT_INDEX=""
TOPOLOGY_MINT_DECIMALS=6
TOPOLOGY_MINT_TREASURY=""
case "$TOPOLOGY" in
  solo)
    NODES="connector:4:14"
    PEERINGS=""
    ;;
  two-hop)
    NODES="connector-a:5:15 connector-b:6:16"
    PEERINGS="a-b:evm:connector-a:connector-b"
    ;;
  mixed-chain)
    NODES="connector-a:7:17 connector-b:8:18 connector-c:9:19"
    PEERINGS="a-b:evm:connector-a:connector-b b-c:solana:connector-b:connector-c:3004"
    ;;
  onion)
    NODES="connector-a:10:20 connector-b:11:21"
    PEERINGS="a-b:evm:connector-a:connector-b"
    ;;
  dealing)
    NODES="connector-a:22:25 connector-b:23:26 connector-c:24:27"
    PEERINGS="a-b:evm:connector-a:connector-b b-c:solana:connector-b:connector-c:3008"
    # The Solana leg settles in this topology's own 9-decimal mock, so it
    # needs its own mint and its own collateral figure. The EVM leg still
    # holds 6-decimal USDC and still deposits CHANNEL_DEPOSIT.
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

# What the payer deposits on a SOLANA leg, which differs from the EVM figure
# only where the two legs hold differently-scaled tokens. Resolved after the
# table so a topology that says nothing gets the one figure both chains used
# before `local/dealing` existed.
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
# Ships in the same bundle as `solana` and `solana-keygen` -- the Solana CLI
# install the local-topologies workflow runs puts `spl-token` on PATH beside
# them, and `make solana-mint-usdc` (which `make local-up` runs before this
# script) already refuses to proceed without it. Named here too because this
# script is runnable on its own: a missing SPL CLI must stop it and say so,
# never leave the Solana settlement accounts silently tokenless (ADR 0007's
# rule for a missing chain binary -- a step that skips and reports success is
# worse than a missing step, because it claims one).
need spl-token "Ships with the Solana CLI; otherwise 'cargo install spl-token-cli'."
need openssl "openssl generates the key material."
need python3 "python3 does the two encodings this script cannot ask a chain tool for."

if [[ ! -x "$CONNECTOR" ]]; then
  echo "ERROR: $CONNECTOR is missing. Run 'cargo build --release -p connector' first --" >&2
  echo "       this script derives the operator allowlist value with it, so the value in" >&2
  echo "       write_keys cannot disagree with whatever actually signs. It derives every" >&2
  echo "       Solana settlement PUBLIC key with it too, for the same reason." >&2
  exit 1
fi

# base58, for the one direction no tool here offers: an ed25519 public key in
# hex is what the connector prints, and base58 is what Solana and every
# committed `[[peer_channels]]` Solana row spell it in.
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
# of `seed || public key`. Built here rather than by `solana-keygen`, which
# only generates random keys and cannot be handed a seed non-interactively. The
# public half is derived by the SAME binary that will sign with it, so an
# account this script funds -- or a mint it creates -- is provably the account
# the connector knows.
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

# The two participants of a Solana channel, in the order the program derives
# its PDA from: `sort_participants` returns (min, max) by the 32-byte VALUE
# (packages/solana-program/src/processor.rs), which base58 does not order --
# sorting the encoded strings gives a different pair and therefore a different,
# perfectly valid-looking, wrong address.
sorted_solana_pair() {
  python3 - "$1" "$2" <<'PY'
import sys
alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def decode(encoded):
    value = 0
    for character in encoded:
        value = value * 58 + alphabet.index(character)
    return value.to_bytes(32, "big")


first, second = sys.argv[1], sys.argv[2]
print(*(
    (first, second) if decode(first) < decode(second) else (second, first)
))
PY
}

# The node's directory, and the committed config file that goes with it. One
# name, two places, and the invariant that keeps them together.
node_dir() { echo "$KEYS/$1"; }
node_config() { echo "$HERE/$TOPOLOGY/$1.toml"; }

# The private key derived for a node, on either chain. `cast wallet
# private-key` prints `0x`-prefixed; every key_file in this repository takes
# bare 64 hex, so the prefix is stripped exactly once, here.
derived_key() {
  cast wallet private-key --mnemonic "$ANVIL_MNEMONIC" --mnemonic-index "$1" | sed 's/^0x//'
}

# Assert `$2` (a committed config) names `$1`. This is the drift guard the
# whole committed-not-generated choice rests on (ADR 0009): every address in
# those files is deterministic, and this is where "deterministic" stops being
# a claim in a comment.
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

# The Solana public key a node signs settlement with, base58 -- the identity
# that IS a channel participant, derived by the binary that will sign with it.
solana_settlement_address() {
  base58 "$("$CONNECTOR" send --operator-key "$(node_dir "$1")/settlement-solana.key" \
    --print-keyid)"
}

# Which SPL mint this topology's Solana channels settle in, and how many
# decimal places it has. Both stages call this, because both derive channel
# PDAs and a PDA is derived FROM the mint -- the same two participants produce
# a different, perfectly valid-looking, wrong address on a different mint.
#
# For every topology but `dealing` the answer is the shared mock USDC mint
# `make solana-mint-usdc` seeds, and nothing else happens here. For a topology
# that owns a mint, the keypair is derived (not generated, not committed) and
# the address it implies is asserted against every committed config that
# settles on Solana -- the same drift guard every settlement address here gets,
# and the thing that makes committing the address legitimate.
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
    IFS=':' read -r node _evm_index _solana_index <<<"$entry"
    config="$(node_config "$node")"
    grep -q '^\[settlement.solana\]' "$config" || continue
    config_must_name "$SOLANA_MINT" "$config" \
      "this topology's own ${TOPOLOGY_MINT_DECIMALS}-decimal mock mint"
  done
}

# The channel PDA the deployed program derives for a Solana peering:
# `find_program_address(["channel", min, max, mint])`
# (packages/solana-program/src/processor.rs). A pure function of the program,
# the mint and the two participants, so it is computed rather than read -- and
# then asserted against both committed configs, which is what makes committing
# it legitimate.
solana_channel_account() {
  local channel_min channel_max
  read -r channel_min channel_max <<<"$(sorted_solana_pair "$1" "$2")"
  solana find-program-derived-address "$SOLANA_PROGRAM_ID" \
    string:channel "pubkey:$channel_min" "pubkey:$channel_max" "pubkey:$SOLANA_MINT" \
    --url "$SOLANA_RPC"
}

# ── The onion topology's one extra step ──────────────────────────────────────
# The hidden-service address does not exist until the daemon has run. `anon`
# generates it into `HiddenServiceDir/hostname` the first time it starts, and
# ADR 0070 decision 7 says what happens next: THE OPERATOR COPIES IT DOWN, into
# `[node]` and into the dialing node's peer `endpoint`. The connector never
# reads that file and never speaks the daemon's control protocol -- both were
# considered and refused, because both couple a node's own self-description to
# a sidecar's filesystem layout.
#
# This function is that operator step, automated exactly as far as it goes: it
# starts the sidecars, reads the address, and writes the two committed configs
# out again with the placeholder substituted. Nothing about the connector
# changes; what changes is who does the copying.
#
# This is the ONE place in this script that talks to docker, and it is the one
# place that has to: no chain CLI, no key file and no committed value can
# supply an address a daemon invents at run time.
onion_compose() {
  # The same two files and the same profiles `make local-up` merges, so this
  # lands in the same compose project (`name: connector`, docker-compose.yml)
  # rather than starting a second stack beside it.
  docker compose -f "$ROOT/docker-compose.yml" -f "$HERE/onion/compose.yml" \
    --profile evm --profile solana --profile onion "$@"
}

render_onion_configs() {
  need docker "The onion topology's address is generated by a container; nothing else has it."

  echo "onion: starting the anon sidecars -- the address does not exist until the daemon has run"
  onion_compose up -d anon-a anon-b

  # The hostname file appears at KEY GENERATION, seconds after start and long
  # before the daemon has bootstrapped -- which is why this waits for the file
  # rather than for the healthcheck. Reachability is compose's `--wait` to
  # enforce, later; what is needed here is only the address.
  local address="" attempt entry
  for attempt in $(seq 1 60); do
    address="$(onion_compose exec -T anon-b \
      cat /var/lib/anon/hidden_service/hostname 2>/dev/null | tr -d '\r\n' || true)"
    # An `if` rather than `[[ ... ]] && break`: under `set -e` a failing
    # AND-list at the end of a loop body takes the whole script down, and the
    # ordinary case here is that the file is not there yet.
    if [[ -n "$address" ]]; then
      break
    fi
    sleep 2
  done

  # `.anyone` and not `.onion`: `anon` renamed the TLD it publishes, and the
  # image this topology builds (local/anon-image) is the release that did. An
  # address read back ending in `.onion` therefore means the sidecar came from
  # somewhere else -- a stale `anon-live:v0.4.10.2` tag on this machine, or a
  # compose file edited to pull the old published image -- and it is worth
  # saying so by name, because the config it would render loads fine and then
  # names an address the daemon beside it cannot resolve (issue #1284).
  if [[ ! "$address" =~ ^[a-z2-7]{56}\.anyone$ ]]; then
    echo "ERROR: anon-b has not produced a v3 .anyone address." >&2
    echo "       Read back: '${address:-<nothing>}'" >&2
    echo "       That address is written by the daemon into" >&2
    echo "         /var/lib/anon/hidden_service/hostname" >&2
    echo "       on its first start. If the container is not running, the usual cause is the" >&2
    echo "       terms flag: 'anon' exits rather than prompting when 'AgreeToTerms 1' is" >&2
    echo "       missing from local/onion/anonrc-b. 'docker compose logs anon-b' says which." >&2
    if [[ "$address" == *.onion ]]; then
      echo "       An address ending in .onion means the sidecar is anon v0.4.9.7, from before" >&2
      echo "       upstream renamed the TLD. This topology builds v0.4.10.2 from" >&2
      echo "       local/anon-image; 'docker image rm anon-live:v0.4.10.2' clears a stale one." >&2
    fi
    exit 1
  fi
  echo "onion: connector-b is reachable at $address"

  for entry in $NODES; do
    IFS=':' read -r node _evm_index _solana_index <<<"$entry"
    local config rendered
    config="$(node_config "$node")"
    rendered="$(node_dir "$node")/connector.toml"
    if ! grep -qF "$ONION_PLACEHOLDER" "$config"; then
      echo "ERROR: $config names no '$ONION_PLACEHOLDER' host, so there is nothing here to" >&2
      echo "       substitute the daemon's address into. Either the placeholder was renamed in" >&2
      echo "       the committed file without renaming it here, or that file stopped being an" >&2
      echo "       onion config -- and rendering it anyway would mount a config naming a host" >&2
      echo "       no circuit reaches." >&2
      exit 1
    fi
    {
      echo "# GENERATED by local/keys.sh from local/$TOPOLOGY/$node.toml -- do not edit."
      echo "# The hidden-service host below was read out of anon-b's own"
      echo "# HiddenServiceDir/hostname and written in here, which is ADR 0070 decision 7:"
      echo "# the daemon generates the address and the OPERATOR copies it down. The"
      echo "# connector reads no hostname file and speaks no control protocol."
      sed "s/$ONION_PLACEHOLDER/$address/g" "$config"
    } >"$rendered"
    chmod a+r "$rendered"
    if grep -qF "$ONION_PLACEHOLDER" "$rendered"; then
      echo "ERROR: $rendered still names the placeholder after substitution." >&2
      exit 1
    fi
    echo "$node: rendered $rendered"
  done
}

# ── Stage two: the Solana peering channels ───────────────────────────────────
# Runs AFTER the connectors are serving, which is the whole reason it is a
# separate stage. The EVM leg below is opened with `cast send` because
# `openChannel` is an ordinary contract call; Solana's `InitializeChannel` is a
# positional account list under an 8-byte discriminator and no chain CLI can
# build one. The only submitter in this repository is a running node's
# `POST /channels` (ADR 0008's third write), which reaches
# `SolanaSettlementBackend::open` and signs with that node's own
# `[settlement.solana]` key -- the very identity the channel's participant is.
#
# So opening it is the OPERATOR's job, after boot, not the connector's at boot:
# ADR 0009's config is read once and creates nothing, and `[[peer_channels]]`
# says which claims a node accepts, not what to go and build on a chain.
#
# FUNDING it is the operator's job for the same reason and by the same route.
# `packages/solana-program`'s `Deposit` credits strictly by signer, so the only
# party that can put the payer's collateral behind the payer's claims is the
# payer's own node -- `POST /channels/:id/fund`, which issue #1118 made a
# self-deposit on both chains. This mirrors the EVM leg's
# `setTotalDeposit(channel_id, payer_address, ...)`; what differs is that no
# chain CLI can stand in for the node here.
#
# `local/open-solana-channel.py` does both writes and then reads the account
# back off the validator, refusing to report success unless the deployed
# program's own account layout agrees with the committed config and the payer's
# own deposit is really there. That assertion is the point: nothing on the peer
# path reads a chain (CF-23), so an unopened -- or opened-but-empty -- channel
# rehearses exactly as green as a real one unless something looks.
open_solana_channels() {
  local found=0
  for peering in $PEERINGS; do
    IFS=':' read -r id chain payer payee port <<<"$peering"
    [[ "$chain" == "solana" ]] || continue
    found=1

    if [[ -z "$port" ]]; then
      echo "ERROR: the '$id' Solana peering names no operator port in this script's topology" >&2
      echo "       table. Its channel is opened through the payer's own operator surface, so" >&2
      echo "       the table has to say which published port that is." >&2
      exit 1
    fi

    local payer_account payee_account channel_account
    payer_account="$(solana_settlement_address "$payer")"
    payee_account="$(solana_settlement_address "$payee")"
    channel_account="$(solana_channel_account "$payer_account" "$payee_account")"
    config_must_name "$channel_account" "$(node_config "$payer")" \
      "the '$id' peering's channel account"
    config_must_name "$channel_account" "$(node_config "$payee")" \
      "the '$id' peering's channel account"

    echo "'$id': opening and funding $channel_account through $payer's operator surface"
    "$HERE/open-solana-channel.py" \
      --rpc-url "$SOLANA_RPC" \
      --operator-url "http://127.0.0.1:$port" \
      --operator-key "$(node_dir "$payer")/operator-send.key" \
      --program-id "$SOLANA_PROGRAM_ID" \
      --token-mint "$SOLANA_MINT" \
      --channel-account "$channel_account" \
      --payer "$payer_account" \
      --payee "$payee_account" \
      --settlement-timeout-seconds 3600 \
      --deposit-base-units "$SOLANA_CHANNEL_DEPOSIT"
  done

  if [[ "$found" == "0" ]]; then
    echo "'$TOPOLOGY' has no Solana peering; nothing to open."
  fi
}

if [[ "$STAGE" == "solana-channels" ]]; then
  if [[ ! -d "$KEYS" ]]; then
    echo "ERROR: $KEYS does not exist. Run 'local/keys.sh $TOPOLOGY' first -- this stage" >&2
    echo "       signs an operator write with a key that stage provisions." >&2
    exit 1
  fi
  resolve_solana_mint
  open_solana_channels
  exit 0
fi

mkdir -p "$KEYS"
chmod 700 "$KEYS"

# Before anything else that touches Solana: which mint this topology's channels
# are derived from and settled in. Both stages ask, and both must get the same
# answer, or the address one opens is not the address the other funds.
resolve_solana_mint

# ── Keys, per node ───────────────────────────────────────────────────────────
for entry in $NODES; do
  IFS=':' read -r node evm_index solana_index <<<"$entry"
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
  # repository accepts (32 raw bytes is the other). Random, and kept across
  # runs: neither appears in a committed file, so nothing depends on their
  # value.
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

  # The two DERIVED keys. Rewritten every run rather than kept, because they
  # are a pure function of the mnemonic and the index: a stale file here would
  # be a settlement address that no longer matches the one a peer's committed
  # config names, which is the single hardest failure in this directory to
  # read backwards from its symptom.
  derived_key "$evm_index" >"$dir/settlement.key"
  derived_key "$solana_index" >"$dir/settlement-solana.key"
  chmod 600 "$dir/settlement.key" "$dir/settlement-solana.key"

  # The Solana CLI's own keypair file, built from the derived seed by
  # `solana_cli_keypair` above -- so the account this script airdrops to is
  # provably the account the connector signs as, and `solana-keygen verify`
  # below is a real signature round trip rather than a re-read of the bytes we
  # just wrote.
  solana_keyid="$("$CONNECTOR" send --operator-key "$dir/settlement-solana.key" --print-keyid)"
  solana_cli_keypair "$dir/settlement-solana.key" "$dir/settlement-solana-cli.json"
  solana_address="$(base58 "$solana_keyid")"
  solana-keygen verify "$solana_address" "$dir/settlement-solana-cli.json" >/dev/null

  # The write allowlist: the PUBLIC half of operator-send.key, one 64-hex key
  # per line. Derived by the binary that will do the signing, so the
  # allowlisted value and the signature cannot disagree.
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

  echo "$node: evm $(cast wallet address --private-key "0x$(cat "$dir/settlement.key")")  solana $solana_address  operator keyid $keyid"
done

# There is no peering secret to generate (ADR 0060). A peering used to need a
# shared `{peerId, secret}` written into both participants' key directories;
# role is now decided by the covering claim's signature against the
# counterparty key `[[peer_channels]]` configures, which is strictly stronger
# and needs nothing symmetric to distribute. The channels below are the whole
# of what a peering needs from this script.

# The connector containers mount these directories READ-ONLY as uid 10001, so
# the files must be world-readable to them. They are mode 600 above for the
# host's sake; relax now that generation is done. These are disposable
# local-chain keys and none of them has ever held value anywhere else.
chmod -R a+rX "$KEYS"

# ── Funding ──────────────────────────────────────────────────────────────────
# The mock USDC must actually be ON the chain before anything mints from it. A
# `cast send` of `mint(...)` to an address with no code does NOT revert -- it
# is an ordinary call to a plain account -- so without this check a race
# against the anvil deploy reports a funded account and leaves an empty one.
# The compose anvil healthcheck gates on the same fact; this is the second
# half, for anyone running the script against a chain they brought up
# themselves.
if [[ "$(cast code "$MOCK_USDC" --rpc-url "$ANVIL_RPC" 2>/dev/null)" == "0x" ]]; then
  echo "ERROR: no contract at $MOCK_USDC on $ANVIL_RPC." >&2
  echo "       The settlement topology is not deployed yet -- DeployLocal.s.sol runs as part" >&2
  echo "       of the compose anvil service's startup. Wait for that container to report" >&2
  echo "       healthy (it gates on exactly this) and re-run." >&2
  exit 1
fi

# The Solana half of the same check, and the signer for it. A throwaway
# `solana` config pointed at the mock mint's authority, so that authority is
# the default signer AND the fee payer of every `spl-token` call below --
# `infra/solana/create-usdc-mint.sh` does the same thing for the same reason
# (spl-token wants its signer flags after the subcommand, and the global config
# sidesteps the placement question entirely). It never touches the developer's
# own `~/.config/solana`.
SOLANA_SPL_CONFIG="$(mktemp)"
trap 'rm -f "$SOLANA_SPL_CONFIG"' EXIT
solana -C "$SOLANA_SPL_CONFIG" config set \
  --keypair "$SOLANA_USDC_AUTHORITY" --url "$SOLANA_RPC" >/dev/null

# A topology that owns its own mint creates it HERE, under that same authority,
# before anything asks what the treasury holds. Idempotent on both halves for
# `create-usdc-mint.sh`'s reasons: the mint is left alone if the validator
# already has it, and the treasury is topped up either way, because the
# validator wipes its state on every start while this script's derived keypair
# does not change.
#
# The airdrop is the authority's fee money. `make solana-mint-usdc` runs before
# this script and does the same thing, so this is only for a hand-run against a
# validator that has not seen that target -- it fails softly because an already
# funded authority is the ordinary case.
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

# The treasury has to actually hold the supply before anything is handed out,
# and the failure mode without this check reads as a shrug: `spl-token
# transfer` against a mint that does not exist reports "Account not found",
# which names neither the mint nor the step that was skipped. The validator
# wipes its state on every start (`infra/solana/entrypoint.sh` passes
# `--reset`), so re-seeding is `make solana-mint-usdc` -- which `make local-up`
# runs before this script, and which fails rather than warns for the same
# reason this does.
if ! treasury_usdc="$(spl-token balance --config "$SOLANA_SPL_CONFIG" \
  "$SOLANA_MINT" 2>&1)"; then
  echo "ERROR: the mock treasury for $SOLANA_MINT on $SOLANA_RPC holds nothing spendable." >&2
  echo "       spl-token said: $treasury_usdc" >&2
  if [[ -n "$TOPOLOGY_MINT_INDEX" ]]; then
    echo "       That mint belongs to the '$TOPOLOGY' topology and is created a few lines above" >&2
    echo "       this check, from $KEYS/mint.json, under the authority" >&2
    echo "       infra/solana/usdc-authority.json. A failure here therefore means the create or" >&2
    echo "       the mint itself failed -- most likely the authority has no SOL for fees on a" >&2
    echo "       validator 'make solana-mint-usdc' was never run against." >&2
  else
    echo "       Mint $SOLANA_MINT is created and seeded by" >&2
    echo "       infra/solana/create-usdc-mint.sh -- run 'make solana-mint-usdc' against a" >&2
    echo "       running validator and re-run this script. Every local connector.toml names" >&2
    echo "       that mint as its [settlement.solana] token_address, so a validator without" >&2
    echo "       it cannot settle at all." >&2
  fi
  exit 1
fi
echo "the $SOLANA_MINT treasury holds $treasury_usdc; $NODE_USDC goes to each node"

for entry in $NODES; do
  IFS=':' read -r node _evm_index _solana_index <<<"$entry"
  dir="$(node_dir "$node")"

  evm_address="$(cast wallet address --private-key "0x$(cat "$dir/settlement.key")")"
  echo "$node: funding EVM settlement account $evm_address"
  cast send --rpc-url "$ANVIL_RPC" --private-key "$ANVIL_ACCOUNT0_KEY" \
    --value 100ether "$evm_address" >/dev/null
  # Mock USDC is MINTABLE (packages/contracts/test/mocks/MockERC20.sol), so
  # this is a mint rather than a transfer out of somebody's balance.
  cast send --rpc-url "$ANVIL_RPC" --private-key "$ANVIL_ACCOUNT0_KEY" \
    "$MOCK_USDC" "mint(address,uint256)" "$evm_address" 1000000000 >/dev/null
  echo "  100 ETH + 1000 USDC (6dp)"

  solana_address="$(solana address --keypair "$dir/settlement-solana-cli.json")"
  echo "$node: funding Solana settlement account $solana_address"
  solana airdrop 100 "$solana_address" --url "$SOLANA_RPC" >/dev/null
  # SOL pays fees; it is not the asset a channel settles in. Without the mock
  # USDC below a node can sign and submit every settlement transaction it likes
  # and still have nothing to put behind its own claims -- which is exactly the
  # state every local node was in until this line existed: the ATA created at
  # boot (`SolanaSettlementBackend::connect` -> `ensure_own_ata_exists`) held
  # zero.
  #
  # A TRANSFER, not a mint: unlike anvil's `MockERC20`, an SPL mint has one
  # authority and this script is not holding it in the loop -- the supply
  # already exists in the treasury `infra/solana/create-usdc-mint.sh` seeded.
  #
  # `--fund-recipient` is required and not belt-and-braces: this stage runs
  # BEFORE any node boots, so the associated token account the transfer lands
  # in does not exist yet, and `spl-token transfer` refuses to create one
  # unless asked. The node's own idempotent create at boot is what would
  # otherwise make this a race rather than a failure.
  #
  # AFTER the airdrop, and that order is load-bearing rather than tidy:
  # `--fund-recipient` still refuses a recipient wallet holding no SOL ("Add
  # `--allow-unfunded-recipient` to complete the transfer"). That flag is
  # deliberately NOT passed -- with the airdrop above it is unnecessary, and
  # without it this line doubles as the confirmation that the airdrop landed.
  spl-token transfer --config "$SOLANA_SPL_CONFIG" --fund-recipient \
    "$SOLANA_MINT" "$NODE_USDC" "$solana_address" >/dev/null
  echo "  100 SOL + $NODE_USDC of $SOLANA_MINT (${TOPOLOGY_MINT_DECIMALS}dp)"
done

# ── The peering channels ─────────────────────────────────────────────────────
# A peer claim is a balance proof against a real payment channel. Nothing on
# the peer path READS the chain to verify one -- `ClaimBook` checks the
# signature against the `counterparty_key` its operator configured and nothing
# else (CF-23) -- so a topology could be rehearsed green against a channel
# that does not exist. It is opened and funded here anyway, for the same
# reason `two_connectors_peer.rs`'s fixture opens and funds one: a claim that
# names a channel nobody could redeem is not a payment, and the difference
# does not show up until someone tries.
for peering in $PEERINGS; do
  IFS=':' read -r id chain payer payee _port <<<"$peering"
  payer_config="$(node_config "$payer")"
  payee_config="$(node_config "$payee")"

  case "$chain" in
    evm)
      token_network="$(cast call "$TOKEN_NETWORK_REGISTRY" "getTokenNetwork(address)(address)" \
        "$MOCK_USDC" --rpc-url "$ANVIL_RPC")"
      config_must_name "$token_network" "$payer_config" "the deployed TokenNetwork"
      config_must_name "$token_network" "$payee_config" "the deployed TokenNetwork"

      payer_key="0x$(cat "$(node_dir "$payer")/settlement.key")"
      payer_address="$(cast wallet address --private-key "$payer_key")"
      payee_address="$(cast wallet address --private-key "0x$(cat "$(node_dir "$payee")/settlement.key")")"
      # Each side's `counterparty_key` is the OTHER side's derived settlement
      # address. This is the assertion the derivation exists for: change an
      # index in the topology table above without changing the committed
      # configs and the failure is here, by name, rather than as a peer claim
      # refused `signature_invalid` after a green start-up.
      config_must_name "$payer_address" "$payee_config" "$payer's EVM settlement address"
      config_must_name "$payee_address" "$payer_config" "$payee's EVM settlement address"

      # The id the committed config names. Read from the file rather than
      # recomputed, so this loop's job is the falsifiable one: make the chain
      # match what is committed, or say why it cannot.
      channel_id="$(sed -n 's/^channel_id = "\(0x[0-9a-f]\{64\}\)"$/\1/p' "$payer_config" | head -1)"
      if [[ -z "$channel_id" ]]; then
        echo "ERROR: $payer_config declares no [[peer_channels]] channel_id, but the topology" >&2
        echo "       table says '$payer' is the payer of the '$id' peering." >&2
        exit 1
      fi
      # `head -1` above is only safe while every `channel_id` line in the file
      # says the same thing. A payer names its channel TWICE since issue #1145
      # -- once in `[[peer_channels]]` for what arrives and once in
      # `[[pay_channels]]` for what it sends, one on-chain channel in both
      # roles -- and if those two ever disagreed this loop would open and fund
      # one of them while the node paid on the other. That is a peering whose
      # claims are cryptographically perfect and worth nothing, which is
      # exactly the class of failure this stage exists to make impossible.
      distinct="$(sed -n 's/^channel_id = "\(0x[0-9a-f]\{64\}\)"$/\1/p' "$payer_config" | sort -u | wc -l)"
      if [[ "$distinct" != "1" ]]; then
        echo "ERROR: $payer_config names $distinct different channel_id values." >&2
        echo "       The '$id' peering holds ONE on-chain channel in two roles: [[peer_channels]]" >&2
        echo "       for the claims that arrive and [[pay_channels]] for the ones it signs. This" >&2
        echo "       stage funds one channel, so two would leave the other's claims unbacked." >&2
        exit 1
      fi
      config_must_name "$channel_id" "$payee_config" "the '$id' peering's channel id"

      state="$(cast call "$token_network" "channels(bytes32)(uint256,uint8,uint256,uint256,address,address)" \
        "$channel_id" --rpc-url "$ANVIL_RPC" | sed -n 2p)"
      if [[ "$state" == "0" ]]; then
        # `openChannel` derives the id as keccak(p1, p2, channelEpoch[p1][p2])
        # with the participants sorted (ADR 0059), so it depends on THIS PAIR
        # and nothing else: another pair's channels on the same TokenNetwork
        # no longer move it. What still moves it is this pair settling a
        # channel, which advances their epoch. That is what the re-read below
        # actually checks -- and the committed ids are all epoch 0, since a
        # local chain is torn down rather than settled.
        cast send --rpc-url "$ANVIL_RPC" --private-key "$payer_key" \
          "$token_network" "openChannel(address,uint256)" "$payee_address" 3600 >/dev/null
        state="$(cast call "$token_network" "channels(bytes32)(uint256,uint8,uint256,uint256,address,address)" \
          "$channel_id" --rpc-url "$ANVIL_RPC" | sed -n 2p)"
        if [[ "$state" == "0" ]]; then
          echo "ERROR: opened a channel between $payer_address and $payee_address on" >&2
          echo "       $token_network, and it did NOT land at the id the committed config names:" >&2
          echo "         $channel_id" >&2
          echo "       That id is keccak(participant1, participant2, channelEpoch[p1][p2]) and the" >&2
          echo "       committed configs all name the epoch-0 id, so this means this pair has" >&2
          echo "       already SETTLED a channel on this TokenNetwork and their epoch has moved" >&2
          echo "       on. 'make local-down && make local-up' resets it." >&2
          exit 1
        fi
        echo "'$id': opened channel $channel_id"
      fi

      # `setTotalDeposit` pulls from the payer's own balance and takes a
      # TOTAL, so this is both the funding and the idempotence.
      cast send --rpc-url "$ANVIL_RPC" --private-key "$payer_key" \
        "$MOCK_USDC" "approve(address,uint256)" "$token_network" "$CHANNEL_DEPOSIT" >/dev/null
      cast send --rpc-url "$ANVIL_RPC" --private-key "$payer_key" \
        "$token_network" "setTotalDeposit(bytes32,address,uint256)" \
        "$channel_id" "$payer_address" "$CHANNEL_DEPOSIT" >/dev/null
      echo "'$id': $payer deposited $CHANNEL_DEPOSIT (6dp USDC) into $channel_id"
      ;;

    solana)
      payer_account="$(solana_settlement_address "$payer")"
      payee_account="$(solana_settlement_address "$payee")"
      config_must_name "$payer_account" "$payee_config" "$payer's Solana settlement key"
      config_must_name "$payee_account" "$payer_config" "$payee's Solana settlement key"

      channel_account="$(solana_channel_account "$payer_account" "$payee_account")"
      config_must_name "$channel_account" "$payer_config" "the '$id' peering's channel account"
      config_must_name "$channel_account" "$payee_config" "the '$id' peering's channel account"
      echo "'$id': channel account $channel_account"

      # OPENED AND FUNDED, but not here: the `solana-channels` stage does both
      # once the payer's node is serving, because `POST /channels` is the only
      # submitter of an `InitializeChannel` this repository has, and
      # `POST /channels/:id/fund` is the only submitter of a `Deposit` signed by
      # the participant being credited (see that stage's own comment). What
      # THIS stage can do is make the address falsifiable -- the two
      # `config_must_name` calls above -- and put the mock USDC in the payer's
      # settlement account for it to collateralise with, which the funding loop
      # above did.
      #
      # The EVM leg deposits here instead only because it can: `setTotalDeposit`
      # names the participant to credit and `cast` can call it, so no running
      # node is needed. Both legs end at the same place -- the payer's own
      # collateral behind the payer's own claims (issue #1118 made `fund` a
      # self-deposit on both chains); they differ in what can submit the
      # transaction, not in what the channel ends up holding.
      #
      # Not always at the same FIGURE, though, and that is ADR 0071 arriving
      # here: `$SOLANA_CHANNEL_DEPOSIT` is `$CHANNEL_DEPOSIT` on every topology
      # whose two legs hold the same 6-decimal token, and a thousand times it
      # on `local/dealing`, whose Solana leg settles in a 9-decimal mock. The
      # same hundred tokens, in the base units of the channel that holds
      # them.
      ;;
  esac
done

# ── The address the daemon generates, copied where the operator would ────────
# Last, because it is the only step that needs a container running rather than
# a chain, and because everything above has to have succeeded before a config
# worth rendering exists. `make local-up` mounts the RENDERED file; the
# committed one is what gets reviewed.
if [[ "$TOPOLOGY" == "onion" ]]; then
  render_onion_configs
fi

echo
echo "keys for '$TOPOLOGY' are in $KEYS (gitignored)"
