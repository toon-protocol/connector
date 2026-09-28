# TASK

Repair pull request #{{PR_NUMBER}} on branch `{{BRANCH}}` so its checks pass and it is
mergeable.

You were dispatched by the factory's PR repair pass (toon-meta#357): this PR's ONLY
blocker(s) are a merge conflict and/or failing checks — every other precondition
(approval, review state, `needs:human`) already holds. Make the smallest change that
gets it green; do not expand scope.

# DIAGNOSE

First, find out exactly why this PR is red:

    gh pr view {{PR_NUMBER}} --json mergeable,statusCheckRollup

- If `mergeable` is `CONFLICTING`, resolve the conflict against `main` (see CONFLICTS
  below).
- For every failing check, read its logs before touching anything:

      gh run view <run-id> --log-failed

  (`<run-id>` is the numeric id in the failing check's `detailsUrl`.)

# CONFLICTS

If the PR conflicts with `main`:

    git fetch origin main
    git merge origin/main

Resolve conflicts by reading BOTH sides and choosing the resolution that preserves both
changes' intent (the same convention `.sandcastle/merge-prompt.md` uses) — never blindly
take "ours" or "theirs". If a conflict needs a judgement call only a human should make,
say so plainly in your final output instead of guessing.

# FAILING CHECKS

This is **connector** — a Rust workspace (`crates/*`, the connector itself) plus an
npm-workspaces monorepo (`packages/*`, devnet tooling). Reproduce the side that failed:

**Rust** (CI's `Rust Workspace Gate`), from the repo root:

- format: `cargo fmt --all -- --check` (the FIRST step — a formatting slip fails the
  gate before your tests ever run)
- build: `cargo build --workspace`
- test: `cargo test --workspace`
- lint: `cargo clippy --workspace --all-targets -- -D warnings`
  (`-D warnings` with `--all-targets` makes dead code a hard error; a shared
  `tests/support/mod.rs` used by several test binaries needs `#![allow(dead_code)]`)

`anvil`/`cast` (Foundry) are in the agent image, so the EVM settlement tests bring up a
real local chain. A chain test that reports `finished in 0.00s` did NOT run — treat
that as a failure, not a pass.

**TypeScript** (`packages/`), ordered — build BEFORE typecheck, or you get phantom
TS2307 errors from unresolved project references:

- lint: `npm run lint --workspaces --if-present`
- build: `npm run build`
- typecheck: `npm run typecheck`
- test: `npm run test --workspaces --if-present`

Other PR checks: `CI / lint-and-format` (`npm run lint`, `npm run format:check`) and
`Agent image` (only on PRs touching `.sandcastle/**`).

Fix the ROOT CAUSE of the failure, not the symptom — do not weaken, skip, delete or
`#[ignore]` a test to get green, and do not loosen a lint threshold. If a failing check
looks like infrastructure flakiness (a CDN, package registry, or setup-step timeout
with no code-level cause), say so plainly in your final output instead of inventing a
change just to make the diff "look different."

# EXECUTION

1. Diagnose the actual cause before editing anything.
2. Make the smallest change that fixes it.
3. Re-run the failing part of the gate locally (in the order above) and confirm it
   passes before you consider the job done.
4. Commit on the current branch (`{{BRANCH}}`) — this is the PR's own branch; do not open
   a new PR.
5. Do not touch anything outside what's needed to turn this PR green.

Once you've made your fix commit(s) (or determined the failure is not fixable from this
branch — say so clearly in your final output), output <promise>COMPLETE</promise>.
