# Mina leaves the repository

**Status:** Accepted — **built** (#1205). Extends [0002](0002-drop-mina-from-the-rust-connector.md), which dropped Mina as a settlement chain but deliberately left the deployed zkApp and its tooling in the tree. This record deletes what that one left standing. Closes #1117. **The number 0065 is shared** with [_A price is a schedule over payload length_](0065-a-price-is-a-schedule-over-payload-length.md), which landed an hour before this record on a branch this one could not see; cite this one as **0065-mina** or by title, and see the [index](README.md) for why neither is renumbered (#1249).

**Scope:** repository scope — what this repository contains, not what any implementation must do. See the [ADR index](README.md).

**Falsifier:** `**/package.json` matching `"o1js"` — every Mina artefact here was reachable only through o1js, so an o1js dependency in any committed manifest means the surface this record deletes has been rebuilt.

**Mina is gone from this repository.** The payment-channel zkApp, the USDC token and its
admin contracts, the browser faucet dApp, the Mina deploy and funding tooling, the devnet
faucet's Mina leg and the treasury workflow are all deleted. What survives is the connector's
refusal of a claim whose `blockchain` is `mina`: that is wire behaviour owed to `toon-client`,
not Mina support, and ADR 0002 remains its record.

## Context

ADR 0002 dropped Mina from the Rust connector because its five zkApp methods need proof
generation through o1js, which exists only in JavaScript, and a Node sidecar beside the binary
was refused. It was careful to scope itself to the connector: "`packages/mina-zkapp` is the
deployed zkApp, which this record never touched."

Fourteen months later nothing consumes what it left. No sibling repository depends on
`@toon-protocol/mina-zkapp`. `packages/mina-usdc-faucet-web` is deployed nowhere — no
workflow, `make` target or infra file publishes it, and its committed `dist/` is the only
build that has ever existed. The devnet faucet's Mina leg has never served a single drip from
the box it currently runs on: it was `503` from the day that box was provisioned (#919), and
funding it needed a human past a bot-check at `faucet.minaprotocol.com`, which is why it
stayed that way.

The cost was not neutral. o1js is why the faucet image is 822 MB and why its box runs a
4 GB plan for a service that serves two HTTP routes and measures 99 MB resident: the Mina leg
compiles zk circuits at boot, for about three minutes. It is why that image is a two-stage
cross-build of another workspace, why the base must be glibc rather than alpine, why the repo
pins Node ≥ 22.12, why `npm run build` had a hand-ordered prefix, and why the one Jest project
in a repository whose gate is `cargo test` existed at all. Every one of those is a fact a
contributor had to learn in order to change something else.

## Considered options

**Keep the zkApp, delete only the faucet leg.** Rejected: it leaves 57 files, the Jest
project, the ordered build and the Node pin in place to serve a package with no consumer. The
cost being paid is the cost of carrying o1js at all, and half a deletion does not stop paying
it.

**Move the zkApp to its own repository.** Rejected as ceremony. Git history holds it, the
deployed zkApp on Mina devnet is unaffected by anything in this tree, and a new repository
nobody builds is the same dead code with more infrastructure around it. If Mina work resumes
it starts from `git show`, and that is a cheaper starting point than a stale repository that
has silently drifted from the deployed contracts.

**Keep `endpoints.json`'s `mina` block as a record.** Rejected: that file is not a record, it
is the live answer to "what does a TOON node point at on devnet", read at runtime. A block for
a chain nothing here settles on is a claim a consumer can act on. History keeps the addresses.

## Consequences

The npm surface is now the faucet, the announcer and `tools/fund-peers`. `jest.config.js` is
deleted with its only project, so each remaining workspace runs its own runner (`node --test`,
`tsx --test`); `npm run build` and `npm run typecheck` are plain `--workspaces` calls with no
hand-ordered prefix; `make clean` is deleted, having only ever removed a Mina build directory;
and the faucet image is one stage. The Node ≥ 22.12 pin stays for now — it is no longer load-
bearing, and removing it is a separate change with its own reasons.

The devnet faucet dispenses USDC on two chains, Base Sepolia and Solana devnet.
`POST /api/mina/usdc-request` is removed from the service, so it answers `404` rather than the
`503` it answered while unconfigured — the same distinction `packages/faucet/test/routes.test.js`
already draws for the native-token routes retired by #945, and it is pinned there.

This unblocks shrinking the faucet box to a 1 GB plan, which is the change that prompted the
record: nothing else on that machine ever needed the memory.

What this record does **not** touch: `crates/` still refuses a `mina` claim by name, with ADR
0002's reason in the error text, and `docs/protocol/*` still specifies that refusal. A claim
arriving from an older client gets the same answer it got yesterday. `infra/linode-store/`'s
`SETTLE_MINA` belongs to the store app's own env contract and is that repository's to retire.
The deleted `docs/mina-deployment.md` and `docs/usdc-mina-inproof-enforcement.md` were the
design records for this work; `docs/usdc-cross-chain-settlement.md` keeps its Mina sections,
now marked as history, because they are the reasoning for a design that shipped and was
retired — exactly what this folder's own conventions say to keep.
