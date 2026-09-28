# The faucet box

Operator runbook for `faucet.devnet.toonprotocol.dev` — the devnet USDC faucet. It is a Linode of
its own, with **no connector on it** (toon-meta#310 §4.5, [connector#898](https://github.com/toon-protocol/connector/issues/898)):
no signer key, no settlement key, no state dir, no payment channel, no `[[peers]]` row anywhere
naming it. It is reached over plain HTTPS, by humans and by client bootstrap code; it is not a
node on the network.

This document covers bringing one up from nothing, and the two operations a live one needs.
The migration story that first put the faucet here — the DNS cutover off the retired apex, and the
Mina leg — is finished and is in git history.

## What it serves

|          |                                                                                    |
| -------- | ---------------------------------------------------------------------------------- |
| Host     | `faucet.devnet.toonprotocol.dev` (Linode label `faucet`, `us-east`)                |
| Plan     | `g6-nanode-1` — 1 vCPU, 1 GB, 25 GB                                                |
| Services | `faucet` (Node, port 3500, not published), `nginx` (80/443), `certbot`             |
| Image    | built **on the box** from `packages/faucet/Dockerfile`; nothing pushes or pulls it |

Two legs, both **USDC only** (§4.6 — no native gas of any chain, ever, from this box):

| Route                            | Chain                  | What happens                                         |
| -------------------------------- | ---------------------- | ---------------------------------------------------- |
| `POST /api/base-sepolia/request` | Base Sepolia (`84532`) | calls the mock USDC's **ungated `mint()`**           |
| `POST /api/solana/usdc-request`  | Solana devnet          | **mints**, because this box holds the mint authority |
| `GET /health`, `GET /api/info`   | —                      | liveness and the capability map                      |

Both legs coin fresh tokens rather than spending a balance, so **neither can run dry and neither
needs topping up**. That is the point of the Solana arrangement, and it is new: until 2026-08 that
leg transferred from a treasury, and it died because the mint's authority key was lost — no one
could refill the treasury and no one could mint. A box that owns its own mint has no such state.

The faucet key holds **no USDC on either chain**. It needs only gas: Base Sepolia ETH, and devnet
SOL for transaction fees and its recipients' ATA rent.

Retired routes answer **404**, not 503: `POST /api/request` (local-anvil EVM),
`POST /api/solana/request` (native SOL), `POST /api/mina/request` and `POST /api/mina/usdc-request`
(the whole Mina leg, [ADR 0065](../adr/0065-mina-leaves-the-repository.md)). 404 vs 503 is the
difference between "removed" and "unconfigured", and `packages/faucet/test/routes.test.js` pins it.

## Who does what

| Step                |        Repo-side (PR, reviewable)        | Human-only (SSH, key material, funds) |
| ------------------- | :--------------------------------------: | :-----------------------------------: |
| 1. Provision        |      ✅ `./devnet-manage.sh faucet`      |     runs it, holds the API token      |
| 2. Certs            | ✅ `bootstrap.sh`, `init-letsencrypt.sh` |         runs them, on the box         |
| 3. Solana CLI       |       ✅ the pinned command below        |          runs it, on the box          |
| 4. Treasury key     |     ✅ `generate-solana-treasury.sh`     |  runs it, on THIS box — never copied  |
| 5. The mint         |     ✅ `create-devnet-usdc-mint.sh`      |         runs it, on THIS box          |
| 6. EVM key + `.env` |                                          |       ✅ generates and funds it       |
| 7. Gates            |         ✅ the curl checks below         |      runs them, reads the output      |
| 8. Resize           |  ✅ `./devnet-manage.sh faucet-resize`   |                runs it                |
| 9. Fleet cutover    |          ✅ the mint-pinning PR          |    lands the pin in each node repo    |

Steps 1, 4, 5, 6 and 8 need SSH, key material, funds or an API token. Every key this box holds is
generated **on it** and never leaves it; nothing here is ever committed.

## Bringing one up

### 1. Provision

`./devnet-manage.sh faucet` creates the Linode (label `faucet`, `NODE_TYPES[faucet]`). No DNS
change: `faucet-cutover` is a separate, deliberate verb.

### 2. Bootstrap and certs

```sh
cd /root && git clone https://github.com/toon-protocol/connector.git && cd connector
cd infra/linode-faucet && cp .env.example .env && $EDITOR .env   # DOMAIN, LETSENCRYPT_EMAIL
./bootstrap.sh
```

`bootstrap.sh` hardens the box first (firewall to 22/80/443, then key-only sshd via
`infra/harden-box.sh`) ahead of anything that can fail, pulls `nginx`/`certbot`, builds the faucet
image, renders `nginx/conf.d/node.conf` for `${DOMAIN}`, starts the stack and runs
`init-letsencrypt.sh`. Set `LETSENCRYPT_STAGING=1` until the public name resolves here, or issuance
fails HTTP-01 and falls back to a self-signed cert; re-run `init-letsencrypt.sh` after DNS moves.

### 3. Install the Solana CLI

`bootstrap.sh` installs docker, git, jq, gettext-base, openssl, ufw, curl and iptables — no Solana
CLI. Steps 4 and 5 need one. Install **v2.1.21**, not `stable` and not a package manager's build:

```sh
sh -c "$(curl -sSfL https://release.anza.xyz/v2.1.21/install)"
export PATH="$HOME/.local/share/solana/install/active_release/bin:$PATH"
```

That version is a choice, not a default: it is the one Solana CLI this repository installs
anywhere, and `crates/connector-settlement-solana/tests/solana_cli_pins.rs` records why and fails
the build if this line names a different one. This box runs no validator and compiles no Rust: the
faucet reaches Solana through `@solana/web3.js` inside its container and never shells out. The CLI
is a bringup tool only, and nothing needs it again after step 5.

### 4. The treasury key

```sh
./generate-solana-treasury.sh          # writes /root/keys/solana-usdc-treasury.json, 0600
```

It generates a fresh keypair at the exact path the compose file bind-mounts, airdrops 2 devnet SOL
for fees, prints only the **public** key, and refuses to overwrite an existing treasury. Never copy
this key from another box: a copied key exists in two places for as long as both do.

### 5. The mint

```sh
./create-devnet-usdc-mint.sh           # prints the new mint address
```

This creates a 6-decimal mock USDC mint whose **mint authority is the treasury from step 4**, then
reads the authority back off the chain to prove it. No initial supply and no freeze authority:
every token that ever circulates will have been dripped.

It refuses to run twice — a second mint would strand every channel opened against the first, and
the fleet configs pin exactly one address.

Put the address it prints into this box's `.env`:

```sh
echo "SOLANA_USDC_MINT=<the address>" >> .env
```

### 6. The EVM key, and `.env`

`BASE_SEPOLIA_FAUCET_KEY` is a fresh EVM private key funded with a little Base Sepolia ETH for gas
— the mock USDC's `mint()` is ungated, so it needs no USDC. Write it into `.env`, then:

```sh
docker compose -f infra/linode-faucet/docker-compose.faucet.yml up -d --build faucet
docker compose -f infra/linode-faucet/docker-compose.faucet.yml logs faucet | tail -20
```

The log must say `Mint authority confirmed`. If this box is pointed at a mint it does not own, it
says so at boot naming **both** keys, and the Solana route answers 503 rather than failing per
request with an SPL error that names no address.

Record how each key was generated somewhere off this box. There is no legacy identity to fall back
on, and no way to recover a lost mint authority — that is the failure this arrangement exists to
avoid repeating.

## Gates

Run in order. **(c) is the one to stop on.**

- **(a) Standalone up.** All three containers healthy, and `GET /health` and `GET /api/info` answer
  through this box's own nginx. Pre-cutover the public name still resolves elsewhere, so pin it:

  ```sh
  curl -ksf --resolve faucet.devnet.toonprotocol.dev:443:<box IP> \
    https://faucet.devnet.toonprotocol.dev/health
  ```

- **(b) Retired legs are gone.** Each of `POST /api/request`, `/api/solana/request`,
  `/api/mina/request` and `/api/mina/usdc-request` returns **404**, not 503.

- **(c) Each configured leg drips, on chain.** Not "returns a signature" — _lands_:

  ```sh
  curl -sf -X POST https://faucet.devnet.toonprotocol.dev/api/solana/usdc-request \
    -H 'content-type: application/json' -d '{"address":"<a throwaway pubkey>"}'
  spl-token balance <mint> --owner <that pubkey> --url https://api.devnet.solana.com   # 1000
  ```

  and the same for `/api/base-sepolia/request`, checked on Basescan. A faucet that answers `200`
  but delivers nothing is the exact failure this gate exists for.

- **(d) Cert renewal survives.** `certbot renew --dry-run`, on this box's own lineage — one
  certificate per name, never a SAN shared with another host.

## Resizing

`./devnet-manage.sh faucet-resize` moves the box to `NODE_TYPES[faucet]`. It refuses any other box,
checks the disk layout can be auto-resized and that the data fits the target plan, then resizes and
waits for `running`.

Linode resizes by shutting the box down, migrating it and booting it: **the faucet is offline for
roughly 10–20 minutes.** The stack comes back on its own (`restart: unless-stopped`); nothing needs
redeploying. Re-run gates (a)–(c) afterwards.

## Cutting the fleet over to a new mint

Only after gate (c) passes on the new mint. The fleet settles against whatever
`[settlement.solana] token_address` its committed config names, so until this lands the two
connector boxes are still using the **old** mint.

1. Land the PR pinning the new address in `infra/linode-relay/connector-rust.toml`,
   `infra/linode-store/connector-rust.toml`, `crates/connector-bin/tests/devnet_configs_load.rs`
   (`FLEET_SOLANA_USDC_MINT`) and `infra/linode/endpoints.json`.
2. Land the same address in each box's OWN committed config — `toon-protocol/relay`'s and
   `toon-protocol/store`'s `deploy/connector.toml`. Step 1 alone moves nothing: since
   [ADR 0068](../adr/0068-a-node-repository-pins-the-connector-nothing-here-moves-a-tag-onto-a-box.md)
   its files are fixtures this repo's tests boot, not what either box reads, and there is no
   `fleet-ops config-apply` any more. Config first, always: the binary and the box's bind-mounted
   TOML are a matched pair in both directions.
3. Drip USDC from this faucet to each box's `[settlement.solana]` address, and check each holds
   SOL for ATA rent.
4. `fleet-health.yml`.

A Solana channel's PDA is seeded with the token mint, so channels opened against the old mint are
invisible to a node configured for the new one. Neither fleet config commits `[[peer_channels]]`,
so there is nothing fleet-side to re-open; client-side channels stay on the old mint until the
consuming repos re-pin, which is why those follow-ups are filed before the cutover lands.

## Rollback

**The plan** is reversible: `faucet-resize` back up, same downtime.

**The mint is not.** Pointing `SOLANA_USDC_MINT` back at the old mint restores nothing — nobody can
mint it, which is why it was replaced. Fleet-side rollback is reverting the previous address in
each node repo's own committed config, which returns the boxes to a mint the faucet cannot
dispense. In practice: fix forward.
