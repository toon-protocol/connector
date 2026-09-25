#!/usr/bin/env bash
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# TOON devnet lifecycle manager — provision, point DNS at, tear down, or probe
# the devnet fleet: Store-DVM / Relay. It no longer deploys to either box; see
# the ADR 0068 note below.
#
# The apex ("toon"/g.toon proxy box) is GONE (issue #872, toon-meta#310 /
# toon-meta#313's live cutover): the fleet is two connector-bearing boxes now
# -- store and relay -- plus the separate, connector-less faucet box
# (connector#898). There is no forwarding hop and no peering left to verify;
# `g.toon` remains the namespace root in the wire protocol but nothing
# answers at it any more, so this script cannot target it either.
#
# The self-hosted EVM/Solana/Mina chain boxes this script once managed were
# deleted 2026-07-19 — the devnet settles on PUBLIC chains now (Base Sepolia,
# public Solana devnet); see infra/linode/endpoints.json's
# own note. Recreating them here would be a live footgun (issue #819), not a
# restoration, so they are gone from this file too.
#
# STALE AS OF 2026-09-25 (toon-protocol/infra#25): the store box this
# describes is gone. Store, gas-station and workload-gateway nodes, plus the
# faucet, were consolidated onto the `relay` box itself — now the devnet's
# one host, running an edge (toon-protocol/infra `edge/deploy`) in front of
# all of them. `./devnet-manage.sh store`/`up`/`destroy` below still
# provision or delete a SEPARATE store Linode; running them now creates or
# destroys a box that plays no part in the deployed devnet. They are left
# as-is (not redesigned) for reference and manual disaster recovery; do not
# run `up`, `store` or `destroy` against the live devnet without checking
# `toon-protocol/infra`'s `docs/devnet.md` first.
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
#   ./devnet-manage.sh up        Provision the store/relay boxes + DNS
#   ./devnet-manage.sh store     Provision + DNS ONLY the store (DVM) box
#   ./devnet-manage.sh relay     Provision + DNS ONLY the relay box
#   ./devnet-manage.sh faucet    Provision ONLY the faucet box (no connector — connector#898)
#   ./devnet-manage.sh faucet-cutover  Repoint faucet.devnet at the faucet box (run AFTER it's live)
#   ./devnet-manage.sh faucet-resize   Resize the faucet box to its NODE_TYPES plan (brief downtime)
#   ./devnet-manage.sh destroy   Delete all Linode boxes (loses chain state)
#   ./devnet-manage.sh status    Probe every public HTTPS endpoint
#   ./devnet-manage.sh ips       Print current box IPs
#   ./devnet-manage.sh dns       Sync Porkbun A-records to current box IPs
#
# ADR 0068 (issue #1213): `up`/`store`/`relay` PROVISION the box and its DNS
# only. Neither box is deployed to FROM THIS SCRIPT any more — the store and
# relay boxes each run their own repository's `deploy/` bundle
# (`toon-protocol/store`, `toon-protocol/relay`), bootstrapped by that repo,
# not by a checkout of this one. `deploy_store_node`/`deploy_relay_node` and
# the `down`/`redeploy` verbs that drove `/root/connector`-checkout-based
# compose stacks on those boxes are GONE — the box they targeted no longer
# runs that stack. The faucet is unaffected: it still deploys from
# `infra/linode-faucet/` in this repo, by hand, per
# `docs/operators/faucet-box-bringup.md`.
#
# There is no `endpoints` verb any more (issue #1135). It printed a JSON
# document assembled from three box labels that no longer exist
# (`toon-devnet-evm`, `toon-devnet-sol`, `toon-devnet-store` -- the store box's
# label is `ario`, see NODE_LABELS below), naming the self-hosted chains this
# header already says were deleted, their mock tokens, the retired
# `proxy.store.` edge, and a Solana payment-channel program id that was never
# deployed to public devnet and was already superseded on the self-hosted
# validator the day it was committed (the id itself, and its provenance, are
# recorded in the guard named below -- it is deliberately not repeated here, so
# there is one copy of a retired literal rather than two).
# `infra/linode/README.md` had already recorded the generator as retired -- this
# copy of it simply outlived that decision in a second file.
# `infra/linode/endpoints.json` is the hand-maintained record; read it, or
# `./devnet-manage.sh ips` for the boxes' current addresses.
# `crates/connector-settlement-solana/tests/solana_program_ids.rs` is what now
# fails the build if any committed file names a program id that is neither of
# the two this repository records.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"            # connector repo root

# ── Load credentials ─────────────────────────────────────────────────────────
# Creds live in ~/.bashrc as export statements; load them whether interactive or not.
for var in LINODE_CLI_TOKEN PORKBUN_API_KEY PORKBUN_SECRET; do
  [ -n "${!var:-}" ] && continue
  val="$(grep -E "^[[:space:]]*export[[:space:]]+${var}=" ~/.bashrc | tail -1 | sed "s/.*=${var}=*//" | tr -d '"' || true)"
  [ -n "$val" ] && export "$var=$val"
done
: "${LINODE_CLI_TOKEN:?Need LINODE_CLI_TOKEN in ~/.bashrc}"
: "${PORKBUN_API_KEY:?Need PORKBUN_API_KEY in ~/.bashrc}"
: "${PORKBUN_SECRET:?Need PORKBUN_SECRET in ~/.bashrc}"

SSH_KEY="${SSH_KEY:-$HOME/.ssh/id_rsa}"
# DOMAIN is the devnet subdomain suffix — baked into nginx server_names, TLS
# cert names and each box's .env for envsubst. It is NOT the Porkbun-registered
# zone: Porkbun's `dns/editByNameType/<domain>/...` needs the bare registered
# domain, and `devnet.toonprotocol.dev` isn't one ("ERROR Invalid domain").
# PORKBUN_DOMAIN is that zone; update_dns() uses it, never DOMAIN (issue #819 —
# as written against DOMAIN, every update_dns call failed).
DOMAIN="${DOMAIN:-devnet.toonprotocol.dev}"
PORKBUN_DOMAIN="${PORKBUN_DOMAIN:-toonprotocol.dev}"
LINODE_API="https://api.linode.com/v4"
PORKBUN_API="https://api.porkbun.com/api/json/v3"

# Node definitions: label | type | root password.
# Live-verified 2026-08-06 against the Linode API (issue #819 comment): the
# fleet was `toon` (apex, label "toon") + `ario` (store, label "ario") + the
# `relay` box (label "relay") — all g6-standard-2. Issue #872 (toon-meta#310 /
# toon-meta#313's live cutover) destroyed the apex; `toon` is no longer a key
# this script can create, deploy, or target. The `store` key's label reads
# "ario", not "toon-devnet-store" — get_box_ip would find nothing under the
# old name and `create_box` would stand up a SECOND store box.
declare -A NODE_LABELS=( [store]=ario [relay]=relay [faucet]=faucet )
# Sizing: the store box was measured on 2026-08-27 using ~125MB of RAM across
# all six of its containers, on a 4GB plan. g6-nanode-1 (1GB, 1 vCPU, 25GB) is
# ample for it and costs $5/mo against $24.
#
# The faucet is a nanode for the same reason and on the same evidence: it runs
# no connector, no chain and no validator — nginx, certbot and a Node process
# serving two HTTP drip routes, measured the same day at 99MB resident and
# 6.2GB of disk. It was sized to match the connector boxes only because its
# Mina leg compiled zk circuits at boot and needed the memory to do it; ADR
# 0065 deleted that leg, and the plan with it. By the time of the infra#25
# devnet audit (2026-09-25) the relay had also become a g6-nanode-1 — this
# map's g6-standard-2 was stale — and it stayed one: the relay's box is now
# the devnet's one host (infra#25), and cutover memory measurements showed
# the whole fleet (every connector at 2-7MB idle, the apps at 28-92MB apiece)
# fits in about 600MB of the nanode's 961MB, so it was never resized.
#
# NOTE: create_box returns early if a box with the label already exists, so
# changing a value here does NOT resize a live box. `./devnet-manage.sh
# faucet-resize` is the path for the FAUCET — deliberately that box only, since
# store and relay each run the connector, which is the client edge on both
# machines (ADR 0041), and resizing one of those is not a thing to reach for a
# generic command to do. Resizing the store is still a hand-made Linode API
# call (POST /linode/instances/<id>/resize with allow_auto_disk_resize) against
# a box whose disk usage already fits the smaller plan.
declare -A NODE_TYPES=(  [store]=g6-nanode-1 [relay]=g6-nanode-1 [faucet]=g6-nanode-1 )
# Root passwords are GENERATED PER CREATE and thrown away — never committed,
# never printed, never reused. Nothing needs them: every path into a box in this
# file is `ssh -i "$SSH_KEY"` (see ssh_run below), and infra/harden-ssh.sh turns
# password authentication off on the box during bootstrap. The Linode API
# requires *a* root_pass on create, so we satisfy it with entropy.
#
# This replaces a hardcoded map of four passwords that lived in this file, in
# this PUBLIC repository, from 2026-06-23. Those values are in git history
# forever, so the boxes that were created with them MUST be rotated on the box
# — changing this file is not sufficient. See
# docs/operators/devnet-ssh-hardening.md.
#
# Linode rejects a password that does not span at least three of {lowercase,
# uppercase, digit, punctuation}. A purely random alphanumeric string misses a
# class often enough to matter (~1 create in 200), and the resulting API error
# would be baffling, so one of each is placed by construction and the rest is
# entropy.
new_root_pass() {
  local body
  body=$(openssl rand -base64 48 | tr -d '\n/+=' | head -c 29)
  # One guaranteed lowercase, uppercase and digit, then shuffle so the fixed
  # classes are not always in the same position.
  printf '%s\n' "a" "R" "7" "$body" | tr -d '\n' | fold -w1 | shuf | tr -d '\n'
}

# ── Linode helpers ─────────────────────────────────────────────────────────
linode_get() { curl -sf -H "Authorization: Bearer $LINODE_CLI_TOKEN" "$LINODE_API/$1"; }
linode_post() { local path=$1; shift; curl -sf -X POST -H "Authorization: Bearer $LINODE_CLI_TOKEN" -H "Content-Type: application/json" "$LINODE_API/$path" "$@"; }
linode_delete() { curl -sf -X DELETE -H "Authorization: Bearer $LINODE_CLI_TOKEN" "$LINODE_API/$1"; }
porkbun() { local path=$1 body=$2; curl -sf -X POST -H "Content-Type: application/json" "$PORKBUN_API/$path" -d "$body"; }

get_box_ip() {  # label → ipv4 or empty
  linode_get "linode/instances" | jq -r ".data[] | select(.label == \"$1\") | .ipv4[0]" 2>/dev/null || true
}
get_box_id() {
  linode_get "linode/instances" | jq -r ".data[] | select(.label == \"$1\") | .id" 2>/dev/null || true
}
get_box_status() {
  linode_get "linode/instances" | jq -r ".data[] | select(.label == \"$1\") | .status" 2>/dev/null || echo "not-found"
}

wait_box_running() {
  local label=$1
  echo "  Waiting for $label to be running..."
  for _ in $(seq 1 60); do
    [ "$(get_box_status "$label")" = "running" ] && return 0
    sleep 5
  done
  echo "ERROR: $label never reached running status" >&2; return 1
}

create_box() {  # key: store|relay|faucet
  local label="${NODE_LABELS[$1]}" type="${NODE_TYPES[$1]}"
  existing_ip="$(get_box_ip "$label")"
  if [ -n "$existing_ip" ]; then
    echo "  $label already exists ($existing_ip) — skipping create"
    return 0
  fi
  echo "  Creating $label ($type)..."
  linode_post "linode/instances" -d "{
    \"type\": \"$type\", \"region\": \"us-east\", \"image\": \"linode/ubuntu24.04\",
    \"root_pass\": \"$(new_root_pass)\",
    \"authorized_keys\": [\"$(cat "$SSH_KEY.pub")\"],
    \"label\": \"$label\", \"tags\": [\"toon-devnet\"], \"booted\": true
  }" | jq -r '"  Created \(.label) → \(.ipv4[0])"'
}

update_dns() {  # subdomain → ip
  local sub=$1 ip=$2
  local auth="{\"apikey\":\"$PORKBUN_API_KEY\",\"secretapikey\":\"$PORKBUN_SECRET\"}"
  local body; body=$(printf '%s' "$auth" | jq ". + {\"name\": \"$sub\", \"type\": \"A\", \"content\": \"$ip\", \"ttl\": \"600\"}")
  local r; r=$(porkbun "dns/editByNameType/$PORKBUN_DOMAIN/A/$sub" "$body")
  if printf '%s' "$r" | grep -q '"SUCCESS"'; then echo "  DNS $sub → $ip"; return; fi
  porkbun "dns/create/$PORKBUN_DOMAIN" "$body" | jq -r '"  DNS \(.status) \("'"$sub"'") → '"$ip"'"'
}

ssh_run() {   # ip, command
  ssh -i "$SSH_KEY" -o StrictHostKeyChecking=no -o ConnectTimeout=30 -o ServerAliveInterval=30 "root@$1" "$2"
}

probe() {  # url, label
  if curl -fsS -m 10 -o /dev/null "$1" 2>/dev/null; then
    printf "  ✅  %-40s %s\n" "$2" "$1"
  else
    printf "  ❌  %-40s %s\n" "$2" "$1"
  fi
}

print_ips() {
  for key in store relay; do
    local label="${NODE_LABELS[$key]}"
    local ip; ip=$(get_box_ip "$label")
    printf "  %-20s %s\n" "$label" "${ip:-not-found}"
  done
}

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
case "${1:-help}" in

up)
  echo "==> [1/2] Provision boxes"
  for key in store relay; do create_box "$key"; done
  for key in store relay; do wait_box_running "${NODE_LABELS[$key]}"; done

  STORE_IP=$(get_box_ip "${NODE_LABELS[store]}")
  RELAY_IP=$(get_box_ip "${NODE_LABELS[relay]}")

  echo "==> [2/2] Update DNS"
  # `proxy.ario` is the store box's paid edge, matching the g.toon.ario
  # prefix it serves. It replaced `proxy.store.devnet` on 2026-08-05; that
  # name is RETIRED -- record deleted, dropped from the certificate and from
  # nginx's server_name. Do not re-add it here: recreating the record would
  # resurrect a name nothing serves, and the next `certbot renew` would still
  # not cover it.
  update_dns "proxy.ario.devnet"    "$STORE_IP"
  update_dns "dvm.devnet"           "$STORE_IP"
  update_dns "proxy.relay.devnet"   "$RELAY_IP"
  # `relay-ws.devnet` points at the RELAY box (#820 / #815), off its own
  # SEPARATELY issued cert lineage (#830 — never bundled into one
  # all-or-nothing SAN request): infra/linode-relay/nginx/conf.d/node.conf +
  # infra/linode-relay/init-letsencrypt.sh. The apex it used to point at
  # (before that split landed) is gone entirely as of issue #872.
  update_dns "relay-ws.devnet"      "$RELAY_IP"

  echo "Boxes provisioned. Deploy is no longer done from this script (ADR 0068):"
  echo "  store -> bootstrap from toon-protocol/store's own deploy/ bundle"
  echo "  relay -> bootstrap from toon-protocol/relay's own deploy/ bundle"
  "$0" status
  ;;

store)
  # Targeted: provision + DNS ONLY the store (DVM) box.
  echo "==> [1/2] Provision store box"
  create_box store
  wait_box_running "${NODE_LABELS[store]}"
  STORE_IP=$(get_box_ip "${NODE_LABELS[store]}")
  echo "==> [2/2] Update DNS"
  update_dns "proxy.ario.devnet"  "$STORE_IP"
  update_dns "dvm.devnet"         "$STORE_IP"
  echo "Provisioned at ${STORE_IP:-<not found>}. Deploy from toon-protocol/store's own deploy/ bundle (ADR 0068)."
  "$0" status
  ;;

relay)
  # Targeted: provision + DNS ONLY the relay box, modeled on `store` above.
  echo "==> [1/2] Provision relay box"
  create_box relay
  wait_box_running "${NODE_LABELS[relay]}"
  RELAY_IP=$(get_box_ip "${NODE_LABELS[relay]}")
  echo "==> [2/2] Update DNS"
  update_dns "proxy.relay.devnet" "$RELAY_IP"
  # relay-ws.devnet belongs to this box too, post-#820 — see the `up)` case's
  # comment for why it used to sit on the apex and what had to land first.
  update_dns "relay-ws.devnet"    "$RELAY_IP"
  echo "Provisioned at ${RELAY_IP:-<not found>}. Deploy from toon-protocol/relay's own deploy/ bundle (ADR 0068)."
  "$0" status
  ;;

faucet)
  # Targeted: provision ONLY the faucet box (toon-meta#310 §4, connector#898).
  # No automatic DNS repoint here either, unlike `store`/`relay` above: this
  # box has no connector (§4.5) and its USDC-only secrets are generated FRESH
  # ON THE BOX by a human (§4.4, never copied from box 1), so there is no
  # `.env` heredoc this script can write the way `store`/`relay` provisioning
  # used to. And per the ordering §6.2 runbook, faucet.devnet
  # must not be flipped at provision time — the record moves only once the
  # NEW box is live, funded and proven serving, which is what stops this
  # command from prescribing the outage the runbook exists to prevent. (It
  # held the record on box 1 until then; that box is gone as of issue #872,
  # so the cutover below has already run.) `./devnet-manage.sh
  # faucet-cutover` (below) is what repoints it — see
  # docs/operators/faucet-box-bringup.md.
  echo "==> [1/2] Provision faucet box"
  create_box faucet
  wait_box_running "${NODE_LABELS[faucet]}"
  FAUCET_IP=$(get_box_ip "${NODE_LABELS[faucet]}")
  echo "==> [2/2] Provisioned at ${FAUCET_IP:-<not found>}"
  echo "Next (human, on the box): clone the repo, cd infra/linode-faucet,"
  echo "cp .env.example .env and fill in fresh keys, then run ./bootstrap.sh."
  echo "See docs/operators/faucet-box-bringup.md."
  ;;

faucet-cutover)
  # Separate, explicit action (toon-meta#310 §6.2 step 9 / toon-meta#313):
  # repoints faucet.devnet at the faucet box. Deliberately its own command,
  # not folded into `up`/`dns`/`faucet` above — run this ONLY after the new
  # box is live, funded and has served real traffic; see the ordered runbook
  # in docs/operators/faucet-box-bringup.md.
  FAUCET_IP=$(get_box_ip "${NODE_LABELS[faucet]}")
  [ -n "$FAUCET_IP" ] || { echo "ERROR: faucet box not found — run './devnet-manage.sh faucet' first." >&2; exit 1; }
  update_dns "faucet.devnet" "$FAUCET_IP"
  ;;

faucet-resize)
  # Move the LIVE faucet box onto NODE_TYPES[faucet]. Deliberately its own
  # verb and faucet-only: `store` and `relay` run the connector, which is the
  # client edge on both machines (ADR 0041), and resizing one of those is not a
  # thing to do by reaching for a generic command.
  #
  # Linode resizes by shutting the box down, migrating it and booting it again
  # — the faucet is OFFLINE for the duration, typically 10-20 minutes. The
  # compose stack comes back on its own (`restart: unless-stopped`); nothing
  # here needs to redeploy it.
  TARGET_TYPE="${NODE_TYPES[faucet]}"
  FAUCET_LABEL="${NODE_LABELS[faucet]}"
  FAUCET_ID=$(get_box_id "$FAUCET_LABEL")
  [ -n "$FAUCET_ID" ] || { echo "ERROR: faucet box not found." >&2; exit 1; }

  CURRENT_TYPE=$(linode_get "linode/instances/$FAUCET_ID" | jq -r '.type')
  if [ "$CURRENT_TYPE" = "$TARGET_TYPE" ]; then
    echo "==> faucet is already $TARGET_TYPE — nothing to do."
    exit 0
  fi

  # Linode refuses a downsize while the box has more disk ALLOCATED than the
  # smaller plan provides -- "Linode has allocated more disk than the new
  # service plan allows", HTTP 400, whatever allow_auto_disk_resize says. The
  # disk has to be shrunk first, and a disk can only be resized while the box
  # is offline. So the order is: measure, power off, shrink, resize, boot.
  #
  # (allow_auto_disk_resize is still passed below. It is what keeps a LATER
  # upsize from leaving the filesystem small, and it costs nothing here.)
  DISKS=$(linode_get "linode/instances/$FAUCET_ID/disks")
  EXT_IDS=$(echo "$DISKS" | jq -r '.data[] | select(.filesystem != "swap") | .id')
  EXT_COUNT=$(echo "$EXT_IDS" | grep -c . || true)
  if [ "$EXT_COUNT" != "1" ]; then
    echo "ERROR: faucet has $EXT_COUNT non-swap disks; this verb shrinks exactly 1." >&2
    echo "Resize its disks by hand in the Linode UI first, then re-run." >&2
    exit 1
  fi
  EXT_ID=$EXT_IDS
  EXT_SIZE=$(echo "$DISKS" | jq -r --arg id "$EXT_ID" '.data[] | select(.id == ($id|tonumber)) | .size')
  SWAP_SIZE=$(echo "$DISKS" | jq '[.data[] | select(.filesystem == "swap") | .size] | add // 0')

  # How much DATA is on the box. Read it from the guest: /disks reports each
  # disk's ALLOCATED size, which is always the whole current plan and so is
  # larger than every smaller plan by construction -- comparing that would
  # refuse every downsize there is.
  TARGET_DISK=$(curl -sf "$LINODE_API/linode/types/$TARGET_TYPE" | jq -r '.disk')
  FAUCET_IP=$(get_box_ip "$FAUCET_LABEL")
  USED_MB=$(ssh_run "$FAUCET_IP" "df -BM --output=used / | tail -1 | tr -dc '0-9'" 2>/dev/null || true)
  if [ -z "$USED_MB" ]; then
    echo "ERROR: could not read disk usage from $FAUCET_IP over SSH." >&2
    echo "Refusing to resize blind: a shrink that does not fit strands the box offline." >&2
    exit 1
  fi

  # Target ext size: the whole new plan minus swap. Keep a margin over what is
  # actually used -- the filesystem needs room to work, and this box builds a
  # container image on itself.
  NEW_EXT_SIZE=$(( TARGET_DISK - SWAP_SIZE ))
  NEEDED_MB=$(( USED_MB * 3 / 2 + 2048 ))
  echo "==> $FAUCET_LABEL uses ${USED_MB}MB of ${EXT_SIZE}MB; $TARGET_TYPE provides ${TARGET_DISK}MB"
  if [ "$NEEDED_MB" -gt "$NEW_EXT_SIZE" ]; then
    echo "ERROR: ${USED_MB}MB used (+50% margin +2GB = ${NEEDED_MB}MB) does not fit ${NEW_EXT_SIZE}MB." >&2
    echo "Free space on the box first, then re-run." >&2
    exit 1
  fi

  echo "==> Resizing $FAUCET_LABEL ($FAUCET_ID): $CURRENT_TYPE -> $TARGET_TYPE"
  echo "    The faucet will be DOWN for roughly 10-20 minutes."

  if [ "$EXT_SIZE" -gt "$NEW_EXT_SIZE" ]; then
    echo "==> [1/4] Powering off (a disk can only be resized offline)"
    linode_post "linode/instances/$FAUCET_ID/shutdown" >/dev/null
    for _ in $(seq 1 60); do
      [ "$(get_box_status "$FAUCET_LABEL")" = "offline" ] && break
      sleep 5
    done
    [ "$(get_box_status "$FAUCET_LABEL")" = "offline" ] || {
      echo "ERROR: $FAUCET_LABEL did not power off; not touching its disk." >&2; exit 1; }

    echo "==> [2/4] Shrinking the root disk ${EXT_SIZE}MB -> ${NEW_EXT_SIZE}MB"
    linode_post "linode/instances/$FAUCET_ID/disks/$EXT_ID/resize" \
      -d "{\"size\": $NEW_EXT_SIZE}" >/dev/null
    # The disk goes `resizing` and back to `ready`; the box stays offline.
    for _ in $(seq 1 120); do
      st=$(linode_get "linode/instances/$FAUCET_ID/disks/$EXT_ID" | jq -r '.status')
      [ "$st" = "ready" ] && break
      sleep 10
    done
    [ "$st" = "ready" ] || { echo "ERROR: disk did not return to ready (status=$st)." >&2; exit 1; }
  fi

  echo "==> [3/4] Resizing the plan"
  linode_post "linode/instances/$FAUCET_ID/resize" \
    -d "{\"type\": \"$TARGET_TYPE\", \"allow_auto_disk_resize\": true}" >/dev/null

  echo "==> [4/4] Waiting for it to come back"
  # A plan resize reboots the box itself when it was running. It was not: we
  # powered it off above, so boot it once the migration settles.
  for _ in $(seq 1 120); do
    st=$(get_box_status "$FAUCET_LABEL")
    [ "$st" = "running" ] && break
    [ "$st" = "offline" ] && linode_post "linode/instances/$FAUCET_ID/boot" >/dev/null 2>&1
    sleep 10
  done
  wait_box_running "$FAUCET_LABEL"

  NEW_TYPE=$(linode_get "linode/instances/$FAUCET_ID" | jq -r '.type')
  echo "==> $FAUCET_LABEL is now $NEW_TYPE"
  echo "    Containers restart themselves; give them a moment, then:"
  probe "https://faucet.$DOMAIN/health" "faucet health"
  echo "    Re-check the drip legs too — /api/info must still report them ready."
  ;;

destroy)
  # Covers the two CONNECTOR-BEARING boxes only. The faucet box (connector#898)
  # is provisioned by its own targeted case above and is not in this loop, so a
  # `destroy` here cannot take the faucet down with the fleet — delete it
  # explicitly if that is what you want. The apex ("toon") this loop once
  # covered too was destroyed live under toon-meta#313 and removed from this
  # script by issue #872 -- there is no key left here for it to target.
  echo "==> Deleting the store/relay devnet boxes (irreversible; NOT the faucet box)"
  read -r -p "Are you sure? [y/N] " ans
  [[ "$ans" =~ ^[Yy]$ ]] || { echo "Aborted."; exit 1; }
  for key in store relay; do
    local_label="${NODE_LABELS[$key]}"
    id=$(get_box_id "$local_label") || continue
    [ -z "$id" ] && echo "  $local_label: not found" && continue
    linode_delete "linode/instances/$id" && echo "  Deleted $local_label ($id)"
  done
  ;;

status)
  echo "Boxes:"
  print_ips
  echo
  echo "Public endpoints:"
  probe "https://faucet.$DOMAIN/health"          "faucet"
  probe "https://relay-ws.$DOMAIN"               "relay-ws"
  # store/relay nginx has no `/health` location -- probing /health on either
  # 404s even when the box is healthy. /ilp/identity is the same
  # unauthenticated liveness read the operator runbook and both boxes' own
  # CORS location use.
  probe "https://proxy.ario.$DOMAIN/ilp/identity"  "store (ario) proxy/connector"
  probe "https://dvm.$DOMAIN/health"               "store dvm"
  probe "https://proxy.relay.$DOMAIN/ilp/identity" "relay proxy/connector"
  ;;

ips) print_ips ;;

dns)
  echo "==> Syncing Porkbun DNS to current box IPs"
  STORE_IP=$(get_box_ip "${NODE_LABELS[store]}")
  RELAY_IP=$(get_box_ip "${NODE_LABELS[relay]}")
  [ -n "$STORE_IP" ] && update_dns "proxy.ario.devnet" "$STORE_IP"  || echo "  ${NODE_LABELS[store]} not found"
  [ -n "$STORE_IP" ] && update_dns "dvm.devnet" "$STORE_IP"        || true
  [ -n "$RELAY_IP" ] && update_dns "proxy.relay.devnet" "$RELAY_IP" || echo "  ${NODE_LABELS[relay]} not found"
  # relay-ws.devnet follows the relay box, post-#820 — see the `up)` case's
  # comment. The apex it used to follow before that split is gone (#872).
  [ -n "$RELAY_IP" ] && update_dns "relay-ws.devnet" "$RELAY_IP"    || true
  echo "Done."
  ;;

help|*)
  # Selected by pattern, not by line number: the header above grows, and a
  # fixed `sed -n '2,10p'` range starts printing prose instead of commands the
  # moment it does.
  echo "TOON devnet lifecycle manager. Usage:"
  grep -E '^#   \./devnet-manage\.sh ' "$0" | sed 's/^#//'
  exit 1
  ;;
esac
