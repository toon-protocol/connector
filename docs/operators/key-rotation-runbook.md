# Key rotation runbook

**Status:** repo-local guidance for rotating live key material. Written for whoever discovers or
suspects an exposure and needs to act, not as a description of any incident in progress.

**No key material is reproduced anywhere in this document, in a PR that links to it, or in any
output produced while following it. Refer to keys by path, box, and role only. Verification below
is by SHA-256 digest, never by printing or pasting the key itself.**

---

## 1. Key classes this repo touches, and where each lives on a box

Every key below is real, funded-or-signing material generated per box (`deploy/connector-rust/
README.md`'s own steps), referenced from a committed config **by file path only** (ADR 0009) —
the path is committed, the file behind it never is (`.gitignore`'s key block; see §3 to check a
new path is covered).

| Key class                     | Role                                                                                                                                          | On-box path (typical)                                                                                                                   | Referenced from                                                                                                                                                          |
| ----------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Signer key                    | This node's ILP identity — signs `GET /ilp/identity`, opens every NIP-59 gift wrap this node terminates (ADR 0012). Holds no funds.           | `signer-rust.key` (repo-relative on the box), mounted to `/app/data/signer.key`                                                         | `[signer] key_file`                                                                                                                                                      |
| Settlement key (EVM)          | Signs on-chain balance-proof redemptions / deposits on Base Sepolia. Holds real (devnet) USDC.                                                | `settlement-rust.key`, mounted to `/app/data/settlement.key`                                                                            | `[settlement.evm.key] key_file` (or legacy flat `[settlement.key]`)                                                                                                      |
| Settlement key (Solana)       | Same role as above, on the deployed `payment-channel` program. 32-byte ed25519 seed, 64 hex chars.                                            | `settlement-solana-rust.key`, mounted to `/app/data/settlement-solana.key`                                                              | `[settlement.solana.key] key_file`                                                                                                                                       |
| Gas-station Solana key        | Store box's kind:5096 fee-payer identity. 128-char hex (raw ed25519 keypair bytes).                                                           | `infra/linode-store/gas-station-solana.key` on the store box, plus `GAS_STATION_SOLANA_SECRET_KEY` in that box's `.env`                 | Box-local tooling, not `connector-rust.toml`                                                                                                                             |
| Peering shared secret         | Bilateral BTP peering credential (issue #750). Both sides need the same bytes.                                                                | `<peer-name>.secret` (e.g. `store-peer.secret`), mounted per the `[[peers]]` entry                                                      | `[[peers]] credential = { secret_file = "..." }`                                                                                                                         |
| Operator write key            | RFC 9421 ed25519 keypair authorizing operator writes (`POST /packets`).                                                                       | `operator.pem` (private half stays with the operator, never on the box); public halves listed in `operator-write-keys.allow` on the box | Public halves only: `operator.write_keys_file` (one 64-hex key per line, `#` comments; issue #1003)                                                                      |
| Operator bearer token         | Read-surface auth (peers, routes, channels, claims, `GET /metrics`, ...).                                                                     | `operator-bearer-token.secret`, mounted to `/app/data/operator-bearer-token`                                                            | `operator.bearer_token_file` (issue #1003 — a path, like every other credential here; the literal `operator.bearer_token` is still accepted but must never be committed) |
| Solana program deploy keypair | Fixes a program's on-chain address across redeploys (`solana program deploy --program-id <keypair>`). Not a signing identity used at runtime. | `crates/<settlement-crate>/deploy/*-keypair.json` (never committed — see `.gitignore` and issue #922)                                   | `solana program deploy` invocations only, not loaded by the running connector                                                                                            |

Config that references a key **by path** is safe to keep public; the file at that path never is.
If a value is ever hand-pasted into a config as a literal instead of a `key_file`/`secret_file`
pointer, that is itself the finding — stop and treat the config as compromised too.

---

## 2. Rotating a key

The shape is the same for every key class above — since issue #1003 that is all of them, the
operator bearer token and write-key allowlist included: generate fresh material, put it on the
box, restart the service that reads it, confirm the old value is dead.

1. **Generate the replacement**, following the same recipe the key was originally created with —
   `deploy/connector-rust/README.md` §1 for the signer key (`openssl rand -hex 32`), the same
   command for settlement/peering secrets (32 random bytes as hex), or the chain's own tooling for
   a settlement account you want a fresh address for (a new EVM/Solana keypair, not just new bytes
   at the same address — rotating the key at the _same_ address does not stop a party who already
   has the old key from continuing to sign for it before the sweep in step 3 completes).
2. **Put it on the box.** A key _file_ (signer/settlement/peering, and since issue #1003 the
   operator bearer token and write-key allowlist too) is never committed, so no workflow here has
   ever written one: copy it directly onto the box (`scp`, matching the pattern in
   `docs/operators/devnet-ssh-hardening.md` §2) at the exact path the config's `key_file`/
   `secret_file` already names. A change to a config file _field_ goes through whichever repo
   deploys that box — for relay and store, their own `deploy/` bundles, since
   [ADR 0068](../adr/0068-a-node-repository-pins-the-connector-nothing-here-moves-a-tag-onto-a-box.md)
   retired this repo's `fleet-ops config-apply`. Set `chmod 600` and `chown 10001:10001` (the container's uid,
   `deploy/connector-rust/README.md` §1) — a key unreadable by that uid fails the service at
   startup, loudly, not silently.
3. **Sweep funded material before rotating a settlement/gas-station key**, not after: send the
   balance to the new address first, so there is no window where the old address is both known-bad
   and still holding value. `connector#659`'s gas-station rotation is the precedent — balance swept
   to a freshly generated address, then the key file replaced.
4. **Restart the service that reads it.** `docker compose restart <service>` on the box for
   anything the running connector reads (signer, settlement, peering) — `fleet-ops.yml`'s
   `restart` operation does this for the faucet box only, the one box this repo still deploys
   (ADR 0068); the box's own gas-station tooling for that key, since it is not read by
   `connector-rust.toml` at all.
5. **Verify** per §4 below before considering the rotation complete.
6. **Confirm the peer side too**, for a peering secret — both ends hold the same bytes, so a
   one-sided rotation just breaks the peering until the other side is updated to match.

Dry-run first: `fleet-ops.yml` and `funded-ops.yml` both default `apply: false`. Read what a dry
run says it would do before re-dispatching with `apply: true`, and quote that dry-run output
(paths and box names, never key contents) in the PR or issue comment recording the rotation.

---

## 3. Checking a candidate path is actually ignored

Before trusting that a new key file will not be committed, prove it against the real ignore rules
rather than assuming a wildcard covers it:

```bash
git check-ignore -v path/to/the/new-key-file
```

A hit prints the `.gitignore` line that matches; no output means the path is **not** covered and
`git add` will happily track it. For a key class this repo doesn't have a wildcard for yet, add
one (matching the existing key block's wildcard style — a new box or a re-derived key must not
need a new rule to stay out of the repo) before generating the key, not after.

This only protects a path that has **never** been tracked. It does nothing for a file that is
already in the index — see §5.

---

## 4. Verifying a rotation landed, without printing the key

Compare the **SHA-256 digest** of the on-box file before and after, never the file's contents.
This is the method `connector#920`'s gas-station rotation used, generalized to any file-backed key
in §1:

```bash
# Before rotating (baseline):
ssh <box> 'sha256sum /app/data/settlement.key'    # or the relevant path

# After copying the new file into place:
ssh <box> 'sha256sum /app/data/settlement.key'
```

A rotation landed iff the digest changed. If it did not, the copy failed silently (permissions,
wrong path, a mount that isn't actually bind-mounting what the config claims) — treat an unchanged
digest as a failed rotation, not a no-op.

For material with an on-chain address (settlement, gas-station), also confirm on chain that the
**old** address no longer holds meaningful value (the sweep from §2 step 3 landed) — a digest
change on the box proves the box moved on, not that the old key is now safe to leave in whatever
git history or logs it may already be in.

Reading the box's config back (by hand over ssh — `fleet-ops.yml`'s `config-read` was retired with
its relay/store legs, ADR 0068) confirms the `key_file`/`secret_file` _path_ and nothing more: a
bind-mounted key file's bytes are not in the config, so this is never a substitute for the digest
comparison above.

---

## 5. Key material found in git history

An ignore rule (§3) stops the _next_ commit from tracking a matching file. It does nothing for one
**already tracked** — that needs an explicit untrack (`git rm --cached <path>`, then commit), which
removes it from the tip but **not from history**: every prior commit that touched the file still
has it, recoverable from any existing clone or fork.

1. **Rotate first, always** (§2), regardless of what else happens to the git history. Rotation is
   what actually closes the exposure — a value still reachable in history is only dangerous while
   it is still live.
2. **Untrack the file at the tip** (`git rm --cached`, `.gitignore` entry, commit) so no _new_
   clone gets a working copy of it, even though history still has it.
3. **Decide on a history rewrite separately, explicitly, and record the decision.** This needs
   push force and (on a public repo with open PRs) coordinating every open branch's rebase — it is
   a human decision (irreversible, affects shared state), not one an agent makes unilaterally. The
   standing decision for the already-rotated devnet key this runbook's own precedent came from is
   recorded on `connector#920`: **not** rewriting, given the key was already rotated, devnet-only,
   and held negligible value — but stated there as a decision to be found, not silently inferred.
   A future exposure may warrant a different answer (mainnet, real funds, a key that cannot be
   rotated out from under an attacker) — don't assume #920's answer generalizes without re-checking
   those facts against the new case.
4. **Confirm the tracked-key CI guard would have caught it**
   (`tools/ci/check-tracked-secrets.sh`, wired into `.github/workflows/ci.yml`'s `tracked-secrets`
   job) — if the file's name doesn't match one of the guard's patterns, that is itself a gap to
   close, not a reason the guard was right to stay silent.

---

## 6. Escalation

Rotating a key with a reviewed workflow (`fleet-ops.yml`, `funded-ops.yml`) needs no human. Stop
and hand the issue back (`needs-triage`, with a comment saying what blocks you) only when the
situation matches this repo's standing escalation rule (`.sandcastle/implement-prompt.md`, "When
you cannot finish"): the action is irreversible with no
rollback (a history rewrite — §5.3), it involves mainnet or real funds, or it needs a
credential/access no reviewed workflow exposes.
