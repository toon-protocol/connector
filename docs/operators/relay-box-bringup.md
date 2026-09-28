# Bringing the relay box up as its own connector

> **Historical as of issue #872** (toon-meta#310 / toon-meta#313's live cutover): this runbook's
> bring-up already happened — the relay box has run its own connector since. Gates (a)-(c) below
> describe proving the apex↔relay peering before the cutover; that peering, and the apex itself,
> are gone. The relay box now terminates `g.toon.relay` directly for its own clients (see
> `docs/devnet-pricing.md`). Kept as the record of how the box was proven safe to bring into the
> fleet.

Operator runbook for [#815](https://github.com/toon-protocol/connector/issues/815) (Shape A: one
connector per box, each fronting its own app — decided on #714). Modeled on
[`btp-peer-transport-bringup.md`](btp-peer-transport-bringup.md)'s "Order" and "Gates" sections,
which is the closest precedent this repo has for standing up a new box's connector and proving a
peering to it before trusting it with paid traffic.

## What is already done, repo-side

- **#816/#823** created `infra/linode-relay/` — `connector-rust.toml` (client-edge only: no
  `[[peers]]`, no `[[peer_channels]]`), `docker-compose.relay.yml` +
  `docker-compose.relay.rust.yml`, `bootstrap.sh`, `firewall.sh`, `init-letsencrypt.sh`,
  `nginx/node.conf.template`, `.env.example`. Mirrors `infra/linode-store/` file-for-file, per
  #815's own instruction to copy the store's shape rather than invent a new one.
- **#818 (this document's sibling deliverable)** decided the price/fee split in
  [`docs/devnet-pricing.md`](../devnet-pricing.md): apex forwards `g.toon.relay` at
  `price = 1, fee = 0`; the relay box terminates it at `price = 1`. `transport = "btp"` moves to the
  relay's own terminating route — already committed there — because it is illegal on the apex's
  `peer_id` forward (`ConfigError::PeerRouteHasTransport`).
- `.gitignore` already covers `infra/linode-relay/*.key` and `*.secret` (#816), so key material
  generated in this runbook is not committable by accident.

## What this runbook does not yet cover

**#820** — the actual peering: opening the on-chain channel between the apex's and relay's
settlement identities, writing `[[peers]]`/`[[peer_channels]]` on both boxes, and flipping the
apex's `g.toon.relay` route from `handler_url` to `peer_id`. This document's "Channel open" and
"Peering flip" steps are what #820 executes; #818's scope is writing this order down and pinning
the numbers, not running the window.

## Who does what

| Step                         |      Repo-side (PR, reviewable)      |    Human-only (SSH, key material, funds)     |
| ---------------------------- | :----------------------------------: | :------------------------------------------: |
| 1. Provision                 |                                      |                  ✅ (#821)                   |
| 2. DNS                       |                                      |                  ✅ (#821)                   |
| 3. Certs                     |       ✅ `init-letsencrypt.sh`       |             runs it, on the box              |
| 4. Key generation/derivation |                                      |     ✅ (#821 — this box's own mnemonic)      |
| 5. Funding                   |                                      | ✅ (#821 — devnet faucet + a human transfer) |
| 6. Standalone verification   |    ✅ `bootstrap.sh`, curl checks    |         runs them, reads the output          |
| 7. Channel open              | ✅ `POST /channels` shape (ADR 0008) |   ✅ holds the bearer token, funds the tx    |
| 8. Peering flip              |         ✅ config PR (#820)          |      ✅ deploys it, restarts both boxes      |
| 9. Rollback                  |       ✅ one-line config edit        |       ✅ applies it, restarts the apex       |

Steps 1, 2, 4 and 5 need SSH, key material or funds this environment does not have — same posture
every other infra-touching ticket in this repo's history records when it applies (#806, #815 §
"Children", #821 itself).

## Preconditions

- `infra/linode-relay/` config, compose files and scripts exist and are reviewed (#816/#823).
- The relay app (`relay:3100`) already runs, and already answers `g.toon.relay` writes today —
  this runbook relocates its front door, it does not touch the app.
- A funded devnet faucet path exists for a **fresh** settlement identity — unlike the store box,
  the relay box has no legacy TypeScript identity to reproduce
  (`infra/linode-relay/connector-rust.toml`'s own header note), so its keys are new material, not
  a mnemonic-index reproduction.
- The apex's `[operator]` surface (ADR 0008, issue #459) is enabled, so step 7 can open a channel
  without hand-crafting a raw settlement transaction. Note what that needs: `bearer_token` gates
  reads, and step 7 is a **write**, so it also needs the private half of a key whose public half is
  on `write_keys` — no shared secret is ever sufficient to move value. Not a given today:
  `infra/linode-node/connector-rust.toml` omits the section deliberately ("Operator surface
  (optional)"), so enabling it per `deploy/connector-rust/README.md` steps 2–3 is itself work this
  precondition asks for.

## Order — provision through peering, in order

1. **Provision.** Stand up a clean Ubuntu/Debian Linode, root SSH reachable. Human-only (#821) —
   no repo change.

2. **DNS.** Point `proxy.relay.${DOMAIN}` and `relay-ws.${DOMAIN}` A-records at the new box's IP —
   the two names `infra/linode-relay/nginx/node.conf.template` renders and
   `init-letsencrypt.sh` requests certs for. Human-only; nothing in the repo depends on DNS having
   propagated yet, but step 3 blocks until it has.

3. **Certs.** `cd infra/linode-relay && cp .env.example .env && $EDITOR .env` (set `DOMAIN`,
   `LETSENCRYPT_EMAIL`, and `LETSENCRYPT_STAGING=1` until DNS is confirmed — `.env.example` ships
   `0`), then `./bootstrap.sh` — it hardens the box first (firewall to 22/80/443 only, then
   key-only sshd; `infra/harden-box.sh`, ahead of everything that can fail), pulls
   images, renders `nginx/conf.d/node.conf` from the template for `${DOMAIN}`, starts the compose
   stack, and runs `init-letsencrypt.sh`, which seeds a self-signed cert, then requests a real one
   for both names once nginx can answer the ACME challenge. If issuance fails it logs a warning and
   falls back to the self-signed cert rather than leaving nginx down — re-run once DNS resolves,
   with `LETSENCRYPT_STAGING=0` for the production cert.

4. **Key generation and derivation.** Three key files, all fresh material from this box's own
   `TOON_MNEMONIC` in `.env`, generated per `deploy/connector-rust/README.md` step 1
   (`openssl rand -hex 32 > …` for a raw signer key, or the mnemonic-derivation path for the
   settlement keys). Write them under `infra/linode-relay/` at the **host** names
   `docker-compose.relay.rust.yml` bind-mounts read-only — `signer-rust.key`,
   `settlement-rust.key`, `settlement-solana-rust.key`, mounted at `/app/data/signer.key`,
   `/app/data/settlement.key` and `/app/data/settlement-solana.key` respectively, which are the
   names `connector-rust.toml` reads and are not the names on disk. **`chown 10001:10001`** each
   file —
   the container runs as uid 10001, and a root-owned `:ro` mount is unreadable to it, which is
   exactly the restart loop #492 hit on the apex's first deploy
   (`failed to read signer key_file …: Permission denied`). Record which derivation path was used
   (mnemonic index, or raw bytes) somewhere off this box, so the identity can be reconstructed if
   the box is lost — there is no legacy identity to fall back to here.

5. **Funding.** Fund the settlement identities derived in step 4 — the EVM address from
   `settlement-rust.key` and the Solana pubkey from `settlement-solana-rust.key` — from the faucet,
   with enough margin to open and fund a channel in step 7 plus headroom for gas. Human-only;
   nothing repo-side verifies a balance before step 7 is attempted, so under-funding surfaces there
   as an ordinary settlement error, not a load-time one.

6. **Standalone verification — no peering yet.** With `./bootstrap.sh` already run in step 3, the
   relay box now answers on its own, exactly the shape `infra/linode-relay/connector-rust.toml`'s
   own header describes: "a legitimately deployable intermediate that answers its own client edge
   and nothing else." Confirm, from outside the box:

   ```sh
   curl -sf https://proxy.relay.${DOMAIN}/ilp/identity   # 200, {"keyId":…,"publicKey":…}
   ```

   and that a write to `g.toon.relay` against this box's own edge is priced and terminated at
   `relay:3100` — an x402 greeting on an unpaid PREPARE, a real settlement on a paid one, at the
   `price = 1` `docs/devnet-pricing.md` now records for this box's own terminate row. This proves
   the box works **standalone** — the same posture `infra/linode-store/` proved before #678 gave it
   a peering — before it is trusted with a peer claim from the apex.

7. **Channel open.** Open a payment channel from the apex's settlement identity to the relay's, via
   the apex's operator surface (ADR 0008, issue #459). **On either chain this step is now `POST
/peers`** (ADR 0075, #1378 on EVM, #1379 on Solana): each box writes `POST /peers { id, url, fee,
max_packet_amount, deposit }` naming the other's URL, which opens and funds that box's own
   outbound x402 channel — on Solana through the other box's sponsor endpoint — and binds the other's
   by its published voucher signer. No channel id is recorded and no `[[peer_channels]]` row is
   written for it ([`operator-spec.md` §1.5](../protocol/operator-spec.md)). The `"chain":"evm"`
   `toon-channel` open and top-up below are refused by name. What follows is only for a **Solana**
   leg declared in the config file's `[[peer_channels]]` rows, which still pays over a
   `toon-channel` until #1380.

   ```sh
   curl -X POST https://proxy.devnet.toonprotocol.dev/channels \
     -H "Authorization: Bearer ${OPERATOR_TOKEN}" \
     -H 'Content-Type: application/json' \
     -d '{"counterparty_hex":"<relay box settlement pubkey, hex>","settlement_timeout_seconds":<…>,"chain":"solana"}'
   ```

   (A node settling on more than one chain refuses an omitted `chain` as ambiguous.)

   **Collateral.** `POST /channels/:id/fund` is a **self-deposit** (issue #1118): run it on the box
   that will _sign_ claims on this channel, and it puts that box's own collateral behind them,
   raising `own_deposited` on `GET /channels`. Each participant funds their own side; neither box
   can fund the other's — `packages/solana-program`'s `Deposit` credits strictly by signer. So: the
   apex funds the apex's side, the relay funds the relay's, and the direction debt actually flows
   decides which of the two matters (§6.4 — debt flows the way packets do). On EVM the deposit is
   `POST /peers`'s own `deposit`, and a top-up is `POST /channels/:id/fund` on the outbound x402
   channel it opened.

   Before the Solana leg can be funded, the box's `[settlement.solana]` address needs the SPL token
   itself, not just SOL for fees: the deposit moves real tokens out of that identity's associated
   token account, which the node creates at boot but nothing fills.

   Record the resulting
   `channel_account` and the relay's `counterparty_key` — exactly the fields `btp-peer-transport-bringup.md`'s "A correct peering"
   example's `[[peer_channels]]` row needs, one row on each side.

8. **Peering flip.** A repo PR (#820), deployed in the same window it merges:
   - **Relay box.** Add `[[peers]]` (accept-only — no `endpoint`, since the apex dials in) and the
     matching `[[peer_channels]]` row from step 7 to `infra/linode-relay/connector-rust.toml`. (An
     explicit `ceiling` was required here per `AcceptOnlyPeerWithoutCeiling` before ADR 0033, issue
     #882, retired both.)
   - **Apex box.** Add `[[peers]]` (the relay's `wss://proxy.relay.${DOMAIN}/ilp/btp` endpoint +
     credential) and the matching `[[peer_channels]]` row to
     `infra/linode-node/connector-rust.toml`. Change the existing
     `[[routes]] prefix = "g.toon.relay"` entry from `handler_url = "http://relay:3100/write"` to
     `peer_id = "apex-relay"` (or whatever id both files agree on), keep `price = 1`, add `fee = 0`
     (`docs/devnet-pricing.md`'s decided split), and **delete** `transport = "btp"` from this
     entry — it is already present on the relay's own terminating route and is a load-time error
     (`PeerRouteHasTransport`) if left on a `peer_id` route.
   - Restart both boxes. This is the step where **the apex loses client-edge BTP enforcement on
     `g.toon.relay`** — recorded in `docs/devnet-pricing.md`, not a surprise this runbook is
     introducing.
   - **Resolve `g.toon.relay.ario` first, not after.** Per `docs/devnet-pricing.md`'s divergence
     section: today it 404s for free at the apex's own `relay:3100`; flipping `g.toon.relay` to a
     forward without also deciding this prefix makes the failure cost a real signed peer claim
     instead of nothing. Add a `g.toon.relay.ario` forward-to-store route, or retire the name,
     in this same PR.

9. **Rollback.** One config edit on the apex, same shape as `btp-peer-transport-bringup.md`'s own
   rollback: point `g.toon.relay` back at `handler_url = "http://relay:3100/write"` (today's live
   shape, restoring `transport = "btp"` on it) and restart. No client-visible change, because
   nothing in discovery names the relay box directly yet.

## Gates — in order, and do not reorder (c)

Reused from `btp-peer-transport-bringup.md`, which is the same proof obligation against a different
pair of boxes:

- **(a) Link up.** BTP auth between apex and relay succeeds both directions; the session survives a
  relay-container restart and reconnects without operator action.
- **(b) Routing intact.** The apex still answers a price for `g.toon.relay`
  (`GET /ilp/routes/price?destination=g.toon.relay` → `1`); a probe of the prefix returns the
  priced reject carrying `toon-accumulated-cost`.
- **(c) Paid write end to end with NO free-write path.** A publish to `g.toon.relay` is charged at
  the apex client edge, forwarded with a peer claim as `payment-channel-claim`, fulfilled by the
  relay box's own `relay:3100`, and the relay-side claim watermark advances. A **claimless** peer
  PREPARE to the route is rejected. This is the **#620 gate — stop the cutover if it fails.** An
  unmetered peer-forwarded route is worse than no peer link at all.
- **(d) Claim exchange complete.** A FLUSH (TRANSFER) sent when traffic stops is acknowledged with a
  `claim-ack` entry on its RESPONSE; a deliberately stale-nonce claim comes back
  `{"result":"rejected","reason":"nonce_not_advancing"}` **without** rejecting the PREPARE it rode
  on. The journaled claim verifies against the configured counterparty and is redeemable.
- **(e) Discovery.** If and when discovery is repointed at the relay box directly (out of this
  runbook's scope — nothing in step 8 changes what `kind:10032` advertises), a `kind:10032` announce
  still resolves to a reachable endpoint for existing clients.

If (c) cannot be demonstrated, stop at step 8 and roll back with step 9 — exactly the posture
`btp-peer-transport-bringup.md` takes for the apex↔store link.

## Rollback

Covered as step 9 above: revert the apex's `g.toon.relay` route to its pre-#820
`handler_url`/`transport = "btp"` shape and restart. The relay box's own peering config can be left
in place — an accept-only peering nobody dials is inert, not harmful — or reverted in the same
commit for a clean diff.
