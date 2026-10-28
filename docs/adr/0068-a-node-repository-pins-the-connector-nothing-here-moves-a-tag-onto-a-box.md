# A node repository pins the connector; nothing here moves a tag onto a box

**Status:** Accepted — built (#1213). Partly supersedes
[0041](0041-a-moving-tag-carries-the-fleets-committed-config-or-it-does-not-move.md)'s Decision 3,
the connector-specific promotion mechanism; Decisions 1, 2 and 4 stand. Supersedes
[0055](0055-a-release-is-one-dispatch-and-the-ordering-rides-as-data.md) in full — it never reached
Accepted, and the promotion regime it specified is retired before it was ever exercised.

**Scope:** deployment law for this fleet — not protocol. See the [ADR index](README.md).

**Falsifier:** `.github/workflows/*.yml` matching `promote-to-fleet\.yml` — this record claims that nothing in this repository calls, dispatches or otherwise names the retired promotion workflow; a match means the mechanism this record retires has come back.

A node repository — `toon-protocol/relay`, `toon-protocol/store` — pins the connector image it
runs, by release handle, in exactly one place in its own `deploy/` bundle, guarded there. This
repository builds the connector and cuts a release; it does not deploy one. `fleet-ops.yml`,
`promote-to-fleet.yml` and `devnet-manage.sh`'s relay/store legs, which used to write a config or
move a tag onto those two boxes, are retired rather than repaired, because there is no longer a
box on the other end that reads what they wrote.

## Context

### What #1213 found

Both the relay box (`97.107.134.182`) and the store box (`ario`, `45.79.173.113`) were re-deployed
on 2026-08-27 onto stacks defined in their _own_ repositories — `toon-protocol/relay`'s and
`toon-protocol/store`'s `deploy/` bundles, each running `docker compose` from a checkout at
`/root/relay` or `/root/store` rather than from `/root/connector`. The relay now runs Caddy, not
nginx, and a `relay-connector` image built by that repo, not this one.

`fleet-ops.yml` still computed, for `box=relay` or `box=ario`:

```
INFRA_DIR=infra/linode-relay          # or infra/linode-store
CONF_REL="$INFRA_DIR/connector-rust.toml"
REMOTE_CONF="$REMOTE_REPO/$CONF_REL"  # -> /root/connector/infra/linode-relay/connector-rust.toml
```

`/root/connector` is a checkout neither box's running container reads any more. `config-apply`
scp'd a config to that dead path, restarted a service (`connector-rust`) neither box's compose
project defines under that name any more, and then **re-read the same dead path to "confirm" the
write.** It passed. The run reported the box was serving the committed config while the box had
not changed at all — worse than a failure, because [0041](0041-a-moving-tag-carries-the-fleets-committed-config-or-it-does-not-move.md)'s
whole thesis is that a moving tag's committed config is _proven_ to have landed, and this had
stopped proving anything while continuing to report green. `restart` and `deploy` were dead the
same way, for the same reason. `promote-to-fleet.yml` booted a build against
`infra/linode-{relay,store}/connector-rust.toml` as "the fleet's committed config" and moved
`:rust-release` on that basis — neither file was what its box ran.

The store side had already been marked as drifting once: [#1203](https://github.com/toon-protocol/connector/pull/1203)
put a `SUPERSEDED` header on `docker-compose.store.yml` naming exactly this gap
("`fleet-ops.yml box=ario` still targets `/root/connector` paths that no longer drive the
deployment") without fixing it, because store#103's migration was the more urgent half of that
change. This record is the fix #1203 flagged and did not make.

### Why repair was rejected

Three shapes were on the table: point `fleet-ops` at each box's real, per-repo path; make it fail
loudly instead of falsely confirming; or retire the verb for these two boxes and let each node
repo own its own deploy.

The first two both assume this repository still has a legitimate write path onto the relay and
store boxes. It does not, by design: `deploy/README.md` already pointed a paid relay or store
operator at that repo's own `deploy/` bundle before this record existed —
_"Local composition of a connector with an app belongs in the app's own repository."_ The relay
repo's bundle pins `CONNECTOR_TAG` in one place, guarded by a test in that repo; the store repo's
does the same (toon-meta#422 tracks landing the pin there). Pointing `fleet-ops` at those repos'
paths would mean this repo's CI holding a write credential into two OTHER repos' deploy state, which
is a wider blast radius than the bug being fixed. Making the confirmation fail loudly is closer, but
it still leaves a verb whose only honest behaviour is "there is nothing here to act on" — the third
option says that in the tool itself, by removing the option to try.

[ADR 0041](0041-a-moving-tag-carries-the-fleets-committed-config-or-it-does-not-move.md) exists to
police an image/config pair that is split across two places — a compose file's `image:` pin here,
a hand-tuned config bind-mounted on a box that trusts it. Once a node repository holds **both** in
its own `deploy/` bundle, guarded by its own test, there is nothing left in _this_ repo for that
policing to do. Retiring the mechanism is not a weaker version of the rule; the rule's premise
stopped holding for these two boxes.

### What does not move with it

The faucet box is unaffected. It has no connector (toon-meta `two-node-architecture.md` §4) and
still deploys from `infra/linode-faucet/` in _this_ repository, built on-box rather than pulled
from a registry Watchtower polls. `fleet-ops.yml`'s `box-status`/`restart`/`deploy` against it are
real operations against a stack this repo still owns, and the file keeps exactly those, narrowed to
that one box.

`fleet-health.yml`'s probe job and its `config-compat` job (which boots `swap:release` against the
committed `infra/linode-relay/swap.config.json`) are untouched by this record — they probe what is
running and validate a maker config unrelated to which repo deploys the connector. ADR 0041
Decisions 1, 2 and 4 continue to bind whatever in this repo's `infra/` still follows a moving tag
under a Watchtower this repo's own compose files declare.

### The consequence for `:rust-release`

Freezing the tag is harmless. The store box, at the time of this record, still runs
`ghcr.io/toon-protocol/connector:rust-release` under its own repo's compose — a transitional state
recorded in `toon-protocol/store`, not here — and its Watchtower simply sees no new digest until
that repo pins a release handle of its own (toon-meta#422). No box is left worse off than it was;
one is left exactly where it was, which is the correct outcome for a tag nothing here has any
further business moving.

## Decision

1. **A node repository pins the connector by release handle, in exactly one place, guarded
   there.** This repository does not compose the connector with an app, and does not hold a write
   path onto a box it does not deploy.
2. **`release-connector.yml` builds and cuts a GitHub Release; it does not promote.** It stays
   `workflow_dispatch`-only (unchanged from ADR 0041 Decision 3 and ADR 0055), loses the
   `config_change_required`/`config_applied_run` inputs and the deploy-ordering step, and its
   release body points a node repo at the `rust-sha-` tag to pin rather than at a promotion
   command.
3. **`.github/workflows/promote-to-fleet.yml` is deleted.** Nothing in this repository moves
   `:rust-release`, or any other tag, onto a box.
4. **`fleet-ops.yml` offers only the faucet box.** Its `relay`/`ario` options and the per-box path
   resolution that served them are removed; `config-read`/`pin-verify`/`config-apply` go with
   them, since the faucet has no connector config for those verbs to act on.
5. **`infra/devnet-manage.sh` provisions boxes and DNS; it does not deploy to relay or store.**
   `deploy_store_node`/`deploy_relay_node` and the `down`/`redeploy` verbs that drove a
   `/root/connector` checkout on those boxes are removed. `up`/`store`/`relay` still create the
   box and sync its DNS record — that much is still this repo's to do — and then say where the
   deploy itself now lives.
6. **`infra/linode-relay/` and `infra/linode-store/` are fixtures, not what a box runs.** Each
   directory says so in a `README.md`, mirroring the header #1203 already put on
   `docker-compose.store.yml`. Neither directory is deleted: `crates/connector-bin/tests/devnet_configs_load.rs`
   still boots the committed `connector-rust.toml` files, checks their self-description and
   settlement identity, and validates their nginx and compose fixtures — real coverage of what
   these files say, decoupled from any claim about which box, if any, currently runs them.
   `devnet_configs_load.rs` drops only the assertions that were specifically about a **deployed**
   compose file: the fleet's connector-image pin of record, the Watchtower label opt-in list, the
   port-binding guard over that file set, and the swap-node moving-tag/CWD checks. Its config-boot,
   self-description and settlement-identity tests, and its nginx upstream-resolution guards, stay —
   none of those claims depended on this repo deploying the file.
7. **`fleet-health.yml` runs on a schedule or a human dispatch only.** Its `workflow_call` trigger
   (which existed only to be called from `promote-to-fleet.yml`) and its `pull_request` trigger are
   removed. Its probe and alert jobs are unchanged.

## Consequences

**A release is smaller and says less.** `release-connector.yml` now does exactly what its own
header says: build, version, publish. Adopting a build is a node repository's own reviewed change
— bumping its pin — not a step this workflow takes on its behalf.

**`:rust-release` is frozen at whatever digest it last held.** See Context above. Nothing here will
move it again; a node repo that still names it is naming a tag this repo no longer supervises, not
a live promotion target. What that means concretely is recorded in the update below.

**The false-green failure mode is gone because the write path is gone.** `fleet-ops.yml` can no
longer report a successful config-apply against a path nothing reads, because it no longer has a
`relay`/`ario` option to do that against.

**Two ADRs are settled rather than left open.** ADR 0055 was Proposed for weeks with a workflow and
a test suite behind it and no release ever cut under it — a real cost of carrying an unbuilt
promotion regime this far. This record closes that question by retiring the regime rather than by
building it out further, which is possible now only because ADR 0041's Decision 3 stopped applying
to a mechanism this repo can no longer reach.

**`crates/connector-bin/tests/fleet_release_gate.rs` shrinks with the mechanism it guarded.** Every
case keyed to `promote-to-fleet.yml`'s content or to `config-change-required` is removed rather than
rewritten to test nothing; what remains — the build workflow never re-publishing `rust-release`,
the release workflow staying dispatch-only, one shared build definition, the release handle's dated
shape, the swap config-compat gate, and the fleet-health probe/alert coverage — are properties that
still hold and are still worth a regression guard.

## Update (2026-08-28) — the tag, named; and the signal, bought back

Two loose ends this record left, closed on the day the fleet finished moving off the tag.

### `:rust-release` is frozen at `rust-sha-8708caf`, and is deliberately not deleted

The tag still exists and still resolves — to the build published from `8708caf`, which **predates
connector#1230**: on it, a peering established by `POST /peers` can accept a claim but can never
sign one, so every packet forwarded over a runtime peering is refused `T00`. Anyone who follows an
older document to `:rust-release` gets a connector that serves, and quietly cannot pay.

As of 2026-08-28 no node repository names it: relay pins its build in `deploy/Dockerfile`'s
`ARG CONNECTOR_TAG`, store and gas-station in their `deploy/docker-compose.yml`, each guarded by
that repo's own bundle test. The "transitionally" clause above is discharged.

**It is not deleted, and the reason is mechanical rather than sentimental.** GHCR has no untag
operation: `rust-release` and `rust-sha-8708caf` are two tags on one package version, and the only
delete the API offers removes the version — which would take the immutable `rust-sha-8708caf` with
it, and with that the rollback target for the build the fleet ran until that morning. An immutable
build tag that vanishes is a worse failure than a retired pointer that misleads, and the second is
addressable by other means.

So the rule is enforced instead of advised. `.github/workflows/fleet-pin-drift.yml` fails — and
opens a `needs:human` issue — if any of the three repositories pins `rust-release`, `rust-main` or
`latest` rather than an immutable `rust-sha-` build. Deleting the tag remains available to a future
operator; what that decision weighs is the loss of `rust-sha-8708caf`, not the tidiness of the tag
list.

### The promotion took a signal with it, and that part was not intended

Retiring `promote-to-fleet.yml` removed this repository's write path onto the boxes, which is the
whole point of this record. It also removed the only moment anything asked whether the fleet agreed
with itself: promotion booted **one** candidate image against **both** boxes' committed configs.
Afterwards, three repositories choose a connector build independently and nothing notices when they
diverge — until a box misbehaves in a way that costs an afternoon to trace back to "these two are
not running the same binary".

`fleet-pin-drift.yml` buys that signal back without buying back the write path. It is read-only and
holds no credential — all four repositories are public, and GHCR is queried with an anonymous pull
token — and it asserts that the three pins parse, name the same build, are immutable, and are
pullable. How far behind `main` the fleet is, it only reports: a pin lagging is what pinning _is_.
The distinction it draws in that report is the one that decides whether a bump is worth making —
commits touching `crates/*/src` change the shipped binary; commits touching tests, docs or fixtures
do not.

This adds no mechanism that moves a tag, writes a config, or reaches a box, and this record's own
falsifier is untouched: nothing here calls, dispatches or names the retired promotion workflow.

**2026-10-28: one pin, not four.** On 2026-10-16 the devnet consolidated to one connector, the
relay's (store#156, gas-station#49, gateway#36). The other three repositories' pins stopped being a
build the devnet runs, and holding them to the relay's raised a standing alarm about a divergence
nothing could feel (#1527). `fleet-pin-drift.yml` now reads only the relay's pin and asserts that it
parses, is immutable and is pullable; the agreement assertion returns if a second connector rejoins
the devnet. The decision above is unchanged: each node repository still owns its pin.
