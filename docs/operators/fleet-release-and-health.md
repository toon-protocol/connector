# Fleet release and health

How a merge becomes a running devnet box, what stops a bad one, and how you find out when
something is down.

Decision records: [ADR 0041](../adr/0041-a-moving-tag-carries-the-fleets-committed-config-or-it-does-not-move.md),
[ADR 0068](../adr/0068-a-node-repository-pins-the-connector-nothing-here-moves-a-tag-onto-a-box.md).
Epic: toon-meta#403.

---

## The shape of it

The three boxes are no longer one shape. The **faucet** box still deploys from this repo
(`infra/linode-faucet/`, built on-box) and this document's release/rollback sections apply to it
only where noted. The **relay** and **store** (`ario`) boxes each deploy the connector from their
own repository's `deploy/` bundle — `toon-protocol/relay`, `toon-protocol/store` — pinning it by
release handle in exactly one place there. **Nothing in this repository moves a tag onto either
box any more** (ADR 0068): `fleet-ops.yml` no longer offers `box=relay`/`box=ario`, and
`promote-to-fleet.yml` is deleted.

`swap`, `store` and `relay` (the apps, not the connector boxes) keep the auto-on-green regime
toon-meta#403 accepted for devnet — a green merge in their own repos reaches the live box within
about a minute, under a label-scoped `containrrr/watchtower:1.7.1` (`--label-enable --interval 60
--cleanup`) each box runs. Watchtower does **no** health gating: it pulls, recreates, and
considers itself done. Whether the process then stayed up, or served anything, is not a question
it asks — which is why the health section below exists and is unaffected by any of this.

## Cutting a connector release

`release-connector.yml` still builds the image, cuts a dated release handle
(`2026.08.21.1` — UTC date, then that day's ordinal, never semver: see
[ADR 0055](../adr/0055-a-release-is-one-dispatch-and-the-ordering-rides-as-data.md)) and opens a
GitHub Release naming the `rust-sha-<short>` tag to adopt. That is now the **whole** job:

```sh
gh workflow run release-connector.yml \
  -f reason="claim-state fix, verified on the relay"
```

Run it **on the commit you want released** — `gh workflow run --ref <branch-or-sha>` — and it must
be on `main`. It is `workflow_dispatch`-only, and stays that way: adding an automatic trigger would
reverse ADR 0041 Decision 3, which is still binding — `connector-rust` is the client edge on both
boxes, so an unreviewed digest reaching either is still a real risk even with no promotion left
here to guard against it.

**Adopting the build is a node repository's own change, not a step here.** Open a PR in
`toon-protocol/relay` or `toon-protocol/store` bumping its pinned connector tag to the `rust-sha-`
tag (or the release's `rust-<handle>` alias) the release names. That repo's own guard — a test that
fails if a second copy of the pin appears anywhere — is what keeps the pin singular; there is no
config-compatibility boot gate here to run first, because the config that pin boots against no
longer lives in this repository.

`:rust-release` is **frozen**. It used to be a promotion tag moved only by an explicit
`promote-to-fleet.yml` dispatch after booting the candidate against both boxes' committed
`connector-rust.toml`; ADR 0068 retired that mechanism because there is nothing left in this repo
for it to check. Do not wire anything to move it — a floating tag moving unsupervised shipped once
(#990) and was reverted, and there is even less reason to repeat it now.

## Keeping the three pins together

Since [ADR 0068](../adr/0068-a-node-repository-pins-the-connector-nothing-here-moves-a-tag-onto-a-box.md)
this repository does not deploy the connector. Three node repositories each pin the build they run,
in one place, guarded by that repo's own bundle test:

| repo          | the pin of record                                    | how a bump reaches the box                                                                   |
| ------------- | ---------------------------------------------------- | -------------------------------------------------------------------------------------------- |
| `relay`       | `deploy/Dockerfile` → `ARG CONNECTOR_TAG`            | the publish workflow rebuilds `relay-connector:release` and Watchtower recreates within ~60s |
| `store`       | `deploy/docker-compose.yml` → `connector:rust-sha-…` | on the box: `git pull && ./render.sh && docker compose up -d`                                |
| `gas-station` | `deploy/docker-compose.yml` → `connector:rust-sha-…` | same                                                                                         |

Bump the pin **and** the literal in that repo's guard test in one reviewed commit — the guards exist
so the two cannot disagree. The pin must be an immutable `rust-sha-` tag: `:rust-release` is retired
and frozen at `rust-sha-8708caf`, a build on which a runtime peering cannot pay (ADR 0068's update).

Land a config change **before** the build that requires it. The parser is `deny_unknown_fields` and
startup is fail-closed, so a schema drift under a box is a refuse-to-start rather than a degraded
run — which is the behaviour you want, and the reason ordering matters.

### The drift check

`.github/workflows/fleet-pin-drift.yml` runs daily and on dispatch. It is read-only, holds no
credential, and reaches no box: it reads the three pins over plain HTTPS and asks GHCR anonymously
whether each is pullable. It **fails**, and opens a rolling `needs:human` issue, when a pin cannot
be parsed, when the three name different builds, when one is a moving tag, or when one cannot be
pulled. The first green run closes the issue.

Being **behind `main` is only reported, never failed** — a pin lagging is what pinning is. The run
summary says how far behind, and whether any of those commits touch `crates/*/src`: if none do, the
shipped binary is unchanged and there is nothing to gain from bumping.

A staged rollout will show as drift while it is in progress. That is correct — it clears when the
last repo lands.

## Rolling the faucet back

The faucet is the one box this repo still redeploys directly:

```sh
gh workflow run fleet-ops.yml -f operation=deploy -f service=faucet -f apply=true
```

For relay, store, and the auto-on-green apps (`swap`, `store`, `relay`), a rollback is that repo's
own concern: for an auto-on-green app, retag its own `:release` onto a known-good `sha-*` build and
let that box's Watchtower pick it up; for the connector on relay/store, bump the pin in
`toon-protocol/relay` / `toon-protocol/store` to an earlier `rust-sha-` build.

---

## What stops a config-breaking change

This is the failure that motivated the connector's promotion regime in the first place, and it is
still worth knowing even though the mechanism it produced is retired. On 2026-08-16 swap#134 added
a **required** `chainProviders[].tokenNetworkAddress`. It merged green, `swap:release` moved,
Watchtower recreated `swap-node`, and the maker crash-looped on `INVALID_CONFIG` — because the
box's bind-mounted `swap.config.json` is not in the image and nobody had added the key. It was down
until a human happened to look.

For `swap` (still auto-on-green, still deployed via a config this repo commits), the rule (ADR 0041) is unchanged:

| Where                                                      | Catches                                                              | When          |
| ---------------------------------------------------------- | -------------------------------------------------------------------- | ------------- |
| `swap`'s `publish-swap-image.yml`, before `:release` moves | a new required key, **in the PR that adds it** — the tag stays put   | pre-deploy    |
| `fleet-health.yml`'s `config-compat` job                   | a mismatch that got in anyway, or a bad edit to the committed config | ≤15 min, cron |

**If you are adding a config key to an app that still deploys against a config this repo commits
(today, only `swap`):** give it a default. If it genuinely has no safe default (swap#134's did not
— defaulting it would have made the maker announce a contract that reverts for every client), then
it is a **breaking deploy**: land the key in the committed config here first, apply it, and only
then merge the app change.

For the connector on relay and store, this discipline is now each node repository's own to keep —
the config a build must boot against lives there, not here.

---

## Health checks and alerts

`.github/workflows/fleet-health.yml` runs every 15 minutes and on demand — schedule or dispatch
only (ADR 0068 removed the `workflow_call` trigger it used to fire after a promotion, along with
the promotion itself). It is strictly read-only on the boxes.

It does **not** take a hardcoded service list. It discovers "every container carrying the Watchtower
enable label" — precisely the set that can change without a human. A labelled service with no probe
defined is a **failure**, not a skip: opting a service into auto-redeploy without saying how to tell
whether it is serving is the omission the file exists to refuse.

Five things are checked, because each catches what the others cannot:

1. **Container state, sampled twice.** A crash-loop shows as `Up 3 seconds` on any single look; a
   rising `RestartCount` across the probe is the giveaway.
2. **A real serving probe.** `Up` is not evidence.
3. **The public edge, from the runner.** This is the one that catches connector#993's stale-nginx
   upstream: a recreate changes the container's Docker network IP, and an nginx that resolved the
   old one 502s to the world while loopback on the box looks perfect. Only an off-box request
   crosses nginx.
4. **A forwarded route, cross-checked.** Every check above asks whether a node is up **for itself**;
   this one asks whether two nodes still agree about the route and the peering between them. Free.
   See "The forwarded route" below.
5. **A forwarded route, actually crossed.** Only a paid packet can prove that. Off by default and
   dispatch-only, because it spends.

### The probes, and why these

| Service           | Probe                                      | Why                                                                                                                                                                                                                                                           |
| ----------------- | ------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `connector-rust`  | `GET 127.0.0.1:4000/ilp/identity` → 200    | The Rust connector has no `/health` — `/health`, `/healthz`, `/status`, `/` all 404, and `/metrics` is 404 on relay but 401 on store. `/ilp/identity` 200s only once the process is serving **and** has read its signer key. `fleet-ops.yml` already uses it. |
| `swap-node`       | `GET 127.0.0.1:8080/health` → 200          | `blsPort`, loopback-published. No public swap health surface exists.                                                                                                                                                                                          |
| `relay`, `store`  | container `HEALTHCHECK` verdict            | These two define one; reading Docker's verdict beats restating their probe here.                                                                                                                                                                              |
| `announce`        | `[announce] OK` in the last 15 min of logs | A loop publisher: no port, no healthcheck. Its printed verdict is the only honest signal. 15 min covers ~3 of its 240s iterations, so one slow publish is not an alert.                                                                                       |
| relay public edge | `proxy.relay…/ilp/identity` → 200          | crosses nginx                                                                                                                                                                                                                                                 |
| relay public edge | `relay-ws…/` → **426**                     | 426 Upgrade Required is the honest liveness signal for a WebSocket-only endpoint. A 200 there would mean something _other_ than the relay is answering.                                                                                                       |
| store public edge | `proxy.ario…/ilp/identity` → 200           | crosses nginx                                                                                                                                                                                                                                                 |

### How you find out

A failing run opens — or comments on — a single rolling issue in this repo:

> **`[fleet-health] devnet fleet is unhealthy`**, labelled `needs:human` + `bug`

`needs:human` is the org's existing human-queue label (toon-meta#347), so the alert lands in a queue
that is already swept rather than inventing a channel of its own. Opening an issue also notifies
everyone watching the repo, which a failed scheduled run does not do reliably — GitHub mails only
the cron's last editor.

The issue carries the full probe table and the rollback commands. **A later green run comments the
recovery and closes it**, so the issue's open/closed state _is_ the fleet's current verdict; you
never have to work out whether an old alert is still live. One issue, not one per failing run: a
fleet that stays down for an hour would otherwise open four.

### The forwarded route: what container health cannot see

Everything above asks whether a node is up and answering **for itself**. Until 2026-08-28 nothing
asked whether one node could still reach another, and that cost two multi-hour outages in one day —
both of them green on every probe in the table above for their whole duration:

|        | what broke                                                                                                        | how long | found by                      |
| ------ | ----------------------------------------------------------------------------------------------------------------- | -------- | ----------------------------- |
| 10:26Z | `T01 peer unreachable` on `g.toon.relay.store` and `g.toon.relay.gas`, after a connector restart on the relay box | ~7 h     | a human sending a job by hand |
| 01:27Z | `T00 … would not report the claim state of channel FDi2TCT9…` on `g.toon.store.relay`, after an SPL mint cutover  | ~12 h    | the same way                  |

The second one is worth understanding, because it is cheap to detect once you have seen it. On
Solana the channel PDA is seeded with the mint — `["channel", min(p1,p2), max(p1,p2), mint]` — so
changing the mint on one box **moves every channel id that box shares with anyone**. Both nodes then
look perfectly healthy in isolation and disagree completely about which channel they are on.

Two jobs close this, and they are deliberately different questions.

#### `peering-crosscheck` — free, every run

Reads each node's public `GET /ilp` self-description (ADR 0050) and holds the three documents to each
other. No ssh key, no bearer token, no packet, **no money**. It also covers the **gas box**, which
nothing else in `fleet-health.yml` touches — the box probes are a matrix over `relay` and `ario`
only.

| Assertion                                                               | What it catches                                                                                                                 |
| ----------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| Every forwarded prefix lands on a node that actually routes it          | a **dangling forward** — a node sells a name nobody routes, and the payer is charged and then refused `F02`                     |
| A forward's price covers the far side's price                           | an **underpriced forward** — one side is repriced, the packet arrives short, and it is refused after the payer has been charged |
| Both sides derive channel ids from the same settlement facts, per chain | the **01:27Z outage**: a mint or token-network change on one box only                                                           |
| At least one side advertises a `peerCarriage`                           | a peering nothing can re-establish after a restart                                                                              |

Exactly one side of a peering dials and the other only accepts, so one side advertising no carriage
is normal and is reported as such. Both advertising none is not.

**What a green cross-check does _not_ mean.** It proves the two sides _agree_. It does not prove a
packet gets through, and nothing free can. The 10:26Z outage is the counter-example: every document
stayed correct for all seven hours — the peering was simply down.

The assertions live in `tools/ci/fleet-peering-crosscheck.py` rather than in the workflow, because
every FAIL branch in them is unreachable from a healthy fleet by construction. `--self-test` drives
all of them against synthetic documents; CI runs it on every PR and the workflow runs it again
before trusting the script against the live fleet. A monitor whose failure branches have never
executed is a green tick over nothing.

#### `paid-probe` — dispatch only, `off` by default

The only check here that spends, and the only one that proves a packet crosses.

```sh
gh workflow run fleet-health.yml -f paid_probe=dry-run   # proves the plumbing, spends nothing
gh workflow run fleet-health.yml -f paid_probe=send      # sends real packets, spends real funds
```

**A forwarded route is a paid route.** An unpaid request to `g.toon.relay.store` comes back as x402
payment terms, which the relay answers out of its own config without consulting the store at all —
so it proves nothing about the peering. The `T01`/`T00`/FULFILL distinction only exists once a packet
has been paid for and forwarded.

**It needs no funded CI wallet, and adds no secret.** The obvious design — a dedicated CI wallet, a
channel opened from CI, a client SDK, a channel-watermark file to reconcile every run — is
unnecessary, because the connector can originate a packet through its own routing: `POST /packets`
(ADR 0008), which `connector send` forms and signs. Driven that way the money comes from **the relay's
own peer channel, the very channel being tested**, and the packet takes exactly the path a client's
would. The job needs only `DEVNET_SSH_KEY`, which this workflow already holds, and the relay box's
own operator write key, which already lives at `/root/relay/deploy/operator-write.key` and never
leaves the box — it is bind-mounted read-only into a throwaway container.

**What it costs, and why arming it is your call.** ~1011 base units for the store leg and ~1001 for
the gas leg: about **$0.002 per run**, out of the relay's peer channels. On the 15-minute cron that
is ~$0.20/day and the channels will need periodic `POST /channels/:id/fund`; hourly is ~$0.05/day.

A **REJECT costs the same as a FULFILL.** ADR 0042 retires ADR 0004's "value moves on fulfilment": a
peer PREPARE carries its covering claim, and the claim is banked on _arrival_, before the packet is
handled. So pointing the probe at an envelope the app refuses makes it cheap in the sense that the
app does no work — no Arweave upload, no gas transaction — and **not** in the sense that the packet is
free. That is the whole reason this is dispatch-only: putting a spend on a cron is a decision about
your own channels' balance, and a workflow edit should not make it for you.

**Reading the answer.** The probe deliberately sends an envelope the terminating app refuses, so the
expected healthy answer is a _final error from the far end_, not a fulfilment:

| Answer                                 | Verdict                                                                                                                                                                                                                                         |
| -------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `FULFILL`                              | crossed, and the far app answered. `connector send` checks the fulfilment against the one its own gift-wrap derives (ADR 0019), so this is proof the packet reached the node holding the shared secret — not merely _a_ node willing to answer. |
| `REJECT F99` (or another far-side `F`) | **pass** — the packet crossed the peering and the far side answered. This is the expected result.                                                                                                                                               |
| `REJECT F02`                           | crossed, but the far side routes no such name. `peering-crosscheck` should have caught this first.                                                                                                                                              |
| `REJECT T01`                           | the peering itself is down; the packet never left. **This is 10:26Z.** The message names the peer.                                                                                                                                              |
| `REJECT T00`                           | the two sides disagree about the channel. **This is 01:27Z.** Read the same run's cross-check settlement rows.                                                                                                                                  |

#### Recovering a peering

A peering is not rolled back — it is re-established, or re-priced, from the operator surface of the
node that owns the row (ADR 0058). Reads need only the bearer token; writes are RFC 9421-signed with
`docs/operators/sign-write.sh`. The relay box ships a copy at `deploy/sign-write.sh` alongside the
private half of its write key; the store and gas boxes hold only the public allowlist
(`operator-write.keys`), so a write to those two is signed from wherever the operator keeps the
private half — which is where it belongs.

```sh
# On the box: what does this node think it forwards, and to whom?
T=$(cat operator-bearer.token)
curl -s -H "Authorization: Bearer $T" http://127.0.0.1:<edge>/routes/peers
curl -s -H "Authorization: Bearer $T" http://127.0.0.1:<edge>/peers

# A DANGLING FORWARD is usually a runtime row the config file no longer owns.
./sign-write.sh -k operator-write.key -X DELETE -p /routes/peers/<prefix>

# A peering whose channel ids moved, or that cannot be dialled, is re-established
# by the same write that made it — ADR 0059 lands on the same channel if one exists.
./sign-write.sh -k operator-write.key -X POST -p /peers \
  -b '{"id":"<peer>","url":"https://…/ilp","fee":1,"max_packet_amount":100000}'

# Then prove a packet crosses, rather than assuming it does.
gh workflow run fleet-health.yml -f paid_probe=send
```

The client edge is `127.0.0.1:4000` on the store and gas boxes and **`127.0.0.1:3000`** on the relay;
the operator surface rides the same port on all three.

### Known gap, not alerted on

`dvm.devnet.toonprotocol.dev` — the store app's public name — currently fails hostname verification.
DNS resolves to the store box and nginx routes the name correctly, but the box serves a certificate
whose only `subjectAltName` is `proxy.ario.devnet.toonprotocol.dev`; it was never reissued to cover
`dvm`. The store app therefore has no working public URL, and its health is observable only on-box.

It is deliberately **not** a fleet-health probe: it would alert forever on a pre-existing certificate
gap rather than on a deploy, which is how a monitor gets ignored. The runbook below closes the gap;
**add the probe only once it is applied**, not before.

## The store box's `dvm.` name has no certificate

### The verdict: reissue the certificate, do not retire the name

`dvm.devnet.toonprotocol.dev` is **not** vestigial, and the evidence was gathered before proposing
either direction (issue #1004):

- The repo already intends it to work, in four committed places: the `map $host $backend` entry
  (`infra/linode-store/nginx/conf.d/node.conf`) that routes it to `store:3400`, both `server_name`
  lines in the same file, the `DOMAINS=(…)` array in `infra/linode-store/init-letsencrypt.sh`, and
  three `update_dns "dvm.devnet" "$STORE_IP"` call sites plus a `status` probe and an `endpoints`
  JSON field in `infra/devnet-manage.sh`. Exactly one thing is out of step — the certificate.
- **The name still serves.** `curl -k https://dvm.devnet.toonprotocol.dev/health` returns the store
  app's live `DvmHealthResponse` (verified 2026-08-16). Only certificate _name verification_ fails,
  so every client that validates — which is all of them — is locked out.
- **It is the store app's only public liveness surface.** `proxy.ario…/ilp/identity` proves the
  _connector_ is up, not the app behind it. Retiring `dvm.` would delete the very thing this
  document calls a gap.
- **It is not a free door.** `store:3400` is the BLS health server, and `startStore`'s Hono app
  registers exactly one route on it: `GET /health`. This is a different port from `store:3300`, the
  payment-oblivious handler that serves `POST /store` — the free door removed on 2026-08-05 (see
  `node.conf`'s own `location /store` gravestone). Putting a valid certificate on `dvm.` exposes a
  read-only health JSON and nothing else.

Retiring it instead would mean edits in six committed places plus a DNS change, in a strict order
(`init-letsencrypt.sh`'s `DOMAINS` **first**, DNS record last — a lineage that lists a name which no
longer resolves fails HTTP-01 for _every_ name on it, taking the live paid edge down at the renewal
mark), to remove a surface nothing else provides. Reissuing costs one certbot run.

### The one repo-side defect this exposed

`init-letsencrypt.sh` issued under `--cert-name "${PRIMARY}"` = `proxy.ario.${DOMAIN}`, while the live
box's `nginx/conf.d/node.conf` loads `/etc/letsencrypt/live/proxy.store.devnet.toonprotocol.dev/`
(the inherited pre-rename lineage, kept on purpose). Running the script on that box as committed
would have issued a correct certificate into a **second** lineage nginx never reads — a silent
no-op. The script now takes a `CERT_NAME` override, defaulting to `PRIMARY` so a fresh box is
unaffected.

### Box commands (operator runs these; all four are on the store box)

```bash
ssh root@45.79.173.113
cd /root/connector
```

**1. Confirm the starting state** — one SAN, and the lineage nginx actually loads.

```bash
docker run --rm -v linode-store_store_certbot_conf:/etc/letsencrypt \
  --entrypoint sh certbot/certbot -c \
  'openssl x509 -noout -subject -dates -ext subjectAltName \
     -in /etc/letsencrypt/live/proxy.store.devnet.toonprotocol.dev/fullchain.pem'
```

Expect `subject=CN=proxy.ario.devnet.toonprotocol.dev` and a `Subject Alternative Name` listing only
`DNS:proxy.ario.devnet.toonprotocol.dev`. If it already lists `DNS:dvm.devnet.toonprotocol.dev`,
stop — the gap is closed and only the nginx reload in step 3 is outstanding.

**2. Expand the existing lineage.** This is `certonly … --expand` rather than
`./infra/linode-store/init-letsencrypt.sh`, deliberately: that script's not-ok path calls
`seed_dummy`, which **overwrites the live `fullchain.pem`/`privkey.pem` with a self-signed pair**
before it deletes and re-requests the lineage. If issuance then failed, the next nginx reload would
serve a self-signed certificate on `proxy.ario…` — the live paid edge. `certonly --expand` never
touches the lineage on disk unless issuance succeeds.

```bash
docker compose -f infra/linode-store/docker-compose.store.yml \
  run --rm --entrypoint certbot certbot \
  certonly --webroot -w /var/www/certbot \
  --cert-name proxy.store.devnet.toonprotocol.dev \
  -d proxy.ario.devnet.toonprotocol.dev \
  -d dvm.devnet.toonprotocol.dev \
  --expand --agree-tos --no-eff-email --non-interactive
```

_What this can destroy:_ on success, the lineage's `live/` symlinks move to a new certificate — the
previous one stays in `archive/` and nothing else on the box is touched. On failure, nothing changes
at all. No container is restarted. `--expand` is required because the name set differs from the
existing certificate's; without it certbot refuses rather than guessing. No `--email`: the account
(`4d89f17f…`) already exists in the volume and passing an address could rewrite it. Do **not** add
`--staging`; the live lineage is production-issued.

Re-run step 1 to confirm two SANs before continuing.

**3. Reload nginx** — it holds the certificate in memory from load time, so step 2 alone changes
nothing that a client sees.

```bash
docker compose -f infra/linode-store/docker-compose.store.yml exec nginx nginx -t
docker compose -f infra/linode-store/docker-compose.store.yml exec nginx nginx -s reload
```

`nginx -t` first: a reload with a bad config leaves the old worker serving, but there is no reason to
find out that way. Neither command restarts the container or drops a connection.

**4. Verify from off-box** (run this from your workstation, not the box):

```bash
curl -sS https://dvm.devnet.toonprotocol.dev/health   # no -k
curl -sS https://proxy.ario.devnet.toonprotocol.dev/ilp/identity
```

The first must return the store's health JSON **without** `-k`. The second is the regression check
that the paid edge still validates on its own name — it shares the lineage, so it is the thing an
expansion could break.

### Follow-up, only after step 4 passes

Add `https://dvm.devnet.toonprotocol.dev/health` to `.github/workflows/fleet-health.yml`'s probe set
(and delete the "deliberately not probed" comment above the probe list), and drop the "Known gap"
section above. Shipping the probe before the certificate is fixed is the failure mode that section
exists to avoid.

### Unrelated defect found while confirming the dependents

`rig`'s `DEVNET_DVM_URL` (`packages/rig/src/cli/name.ts`) defaults `--via` to
`https://dvm.devnet.toonprotocol.dev` for `rig name buy` / `rig name set` on devnet, and posts to
`${via}/store`. That path cannot work through this hostname even with a valid certificate: `dvm.`
maps to `store:3400`, the health server, while `POST /store` is served on `store:3300` and is not
exposed under any hostname (it was deleted as a free door on 2026-08-05). Fixing the certificate does
not fix the brokered ArNS buy. Tracked as toon-protocol/rig#101, which reaches the same conclusion
from the connector side — no node serves an unpaid `POST /store`, by design — and where this box's
half of the evidence is recorded.
