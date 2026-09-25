# TASK

Fix issue {{TASK_ID}}: {{ISSUE_TITLE}}

Pull in the issue. **Use the `--json` form** — a bare `gh issue view <ID>` FAILS on
this repository:

```
gh issue view <ID> --json title,body,labels --jq '"# " + .title + "\n\n" + .body'
```

A classic Project is attached, so any porcelain `gh issue view` / `gh pr view`
call dies with `GraphQL: Projects (classic) is being deprecated ...
(repository.issue.projectCards)` and prints nothing else. The `--json` form takes
a different code path and works. Issue comments are on the REST API, not that
command:

```
gh api repos/toon-protocol/connector/issues/<ID>/comments --jq '.[].body'
```

Read the comments as well as the body — corrections, blockers and decisions
frequently live there rather than in the original description.

If the issue has a parent PRD, pull that in the same way. Do not proceed on the
title alone; if you cannot read the body, say so and stop rather than guessing at
the acceptance criteria.

Only work on the issue specified.

Work on branch {{BRANCH}}. Make commits and run tests.

# CONTEXT

Here are the last 10 commits:

<recent-commits>

!`git log -n 10 --format="%H%n%ad%n%B---" --date=short`

</recent-commits>

# DECIDING THINGS

Most questions that feel like they need a human have already been answered in this repository.
Look before you escalate.

**`docs/adr/README.md` is the decision authority.** It groups every ADR by scope, so you can go
to the right ones instead of reading all of them:

- **Connector architecture** — how this codebase is built. Read these when changing structure,
  config, the operator surface, testing approach, or state handling.
- **Protocol law** — binds every implementation, not just this one. Read these when touching the
  wire, pricing, claims, payloads, or anything another implementation could observe. If your
  change would make a client SDK wrong, it is in this group.
- **Fleet and operations** — deployment, migration, and how other repos are regarded.

**The tiebreaker is ADR 0021: vectors are normative, prose is not.** When a spec document and the
committed vectors disagree, the vectors win, and the prose is the thing to fix. `docs/protocol/`
is explicitly non-normative — it is the readable account of decisions the ADRs own.

**Line numbers in older documents drift.** A `file.rs:123` citation in an issue or spec may point
somewhere else by the time you read it. Verify the reference resolves to what the text claims
before you rely on it; if it does not, find the current location and say so in your PR rather
than following the stale one.

## When a ticket needs a live box, a funded key, or an on-chain write

You cannot do those from this sandbox, and that is deliberate — no credential for a box, a
treasury, or a chain is ever passed into a container running agent-authored code. **This does not
mean the ticket needs a human.** Two reviewed workflows exist to perform exactly this class of
operation, and your token can dispatch them:

- **`.github/workflows/fleet-ops.yml`** — live faucet work, on the one devnet host (infra ADR
  0001): `box-status` (read), and `restart`, `deploy` (writes). It offers nothing else: relay,
  store, gas-station and gateway each deploy from their own repositories now (ADR 0068).
- **`.github/workflows/funded-ops.yml`** — EVM channel work needing a key that can sign and pay:
  `whoami`, `channel-status` (reads), and `deposit` (write).

Dispatch with `gh workflow run <file> -f key=value …`, then read the run's summary back with
`gh run view --log`. **Both default to `apply: false`** — run the dry run first, read what it says
it would do, and only then re-dispatch with `apply: true`. Quote the dry-run output in your PR
description so a reviewer can see what you checked before writing.

If a needed operation has no verb, the right move is to add the verb to the workflow in a PR —
that is a reviewable change — not to ask for a credential.

## When escalation IS right

Add `needs:human` and stop when, and only when:

- the decision is genuinely new — nothing in the ADRs covers it, and reasonable engineers would
  disagree about the answer;
- the action is **irreversible** and no rollback exists;
- it involves **mainnet or real funds** — devnet and testnet do not count;
- it needs a credential or physical access that no reviewed workflow exposes.

"This looks risky" or "I am not sure" is not escalation-worthy on its own. Say what you checked,
what the ADRs say, and what you concluded.

# EXPLORATION

Explore the repo and fill your context window with relevant information that will allow you to complete the task.

Pay extra attention to test files that touch the relevant parts of the code.

# EXECUTION

If applicable, use RGR to complete the task.

1. RED: write one test
2. GREEN: write the implementation to pass that test
3. REPEAT until done
4. REFACTOR the code

# FEEDBACK LOOPS

The gate below is ENFORCED. After you finish, the runner executes it itself and
will not open a PR if it is red — you get a bounded number of fix iterations with
the exact failure output, and then the run fails. So running it yourself as you
go is not a formality; it is the difference between finishing and being handed
your own compile error.

## Rust — `crates/`, the #409 rewrite

If you touched anything under `crates/`, `Cargo.toml` or `Cargo.lock`, run these
from the repo root. They are exactly what CI's `Rust Workspace Gate` runs:

- format: `cargo fmt --all -- --check`
- build: `cargo build --workspace`
- test: `cargo test --workspace --exclude payment-channel`
- lint: `cargo clippy --workspace --exclude payment-channel --all-targets -- -D warnings`

`anvil` and `cast` (Foundry v1.7.1) are installed in this container, so the EVM
settlement tests bring up a real local chain and genuinely run. Per ADR 0007 that
is the design: each integration test spawns its own disposable chain. A chain
test that reports `finished in 0.00s` did NOT run — treat that as a failure and
find out why, do not report it as passing.

Two clippy notes that have bitten before: `-D warnings` with `--all-targets`
makes dead code a hard error, so a shared `tests/support/mod.rs` used by several
test binaries needs `#![allow(dead_code)]`; and `cargo fmt` is the FIRST step, so
a formatting slip fails the gate before your tests ever run.

## TypeScript — `packages/`

connector is an npm-workspaces monorepo. If you touched anything under
`packages/`, run connector's real gate from the repo root and make sure every
command passes:

- lint: `npm run lint --workspaces --if-present`
- typecheck: `npm run typecheck`
- build: `npm run build`
- test: `npm run test --workspaces --if-present`

One connector-specific gotcha when running the test gate:

- The npm workspaces that remain are devnet tooling only (the faucet, the
  announcer, `tools/fund-peers`). The connector itself is Rust — the Rust gate
  is the one that matters for connector changes.

Do not commit until the gates that apply to what you changed all pass.

If a gate fails and you cannot fix it, say so plainly and explain what is
blocking you. Do not weaken, skip, delete or `#[ignore]` a test to get green, and
do not loosen a lint threshold — a gate that was made to pass proves nothing, and
the whole reason it is enforced from outside is that self-reported success is not
evidence.

# COMMIT

Make a git commit. The commit message must:

1. Start with `RALPH:` prefix
2. Include task completed + PRD reference
3. Key decisions made
4. Files changed
5. Blockers or notes for next iteration

Keep it concise.

# THE ISSUE

If the task is not complete, leave a comment on the issue with what was done.

Do not close the issue - this will be done later.

Once complete, output <promise>COMPLETE</promise>.

# FINAL RULES

ONLY WORK ON A SINGLE TASK.

## Context budget

Operate as if your context is capped at **~200k tokens**, whatever your model's actual window
is (org policy: toon-meta's `CLAUDE.md` → _Context budget policy_ — the cap is absolute, not a
percentage of the window, because a percentage means different things on different models).
Treat ~200k as a hard ceiling, not a target, and do the real work well below it.

Start preparing a handoff at roughly **120k** tokens of context, and hand off no later than
roughly **160k** — never run to the ceiling. Handing off means: write a structured handoff note
(goal and remaining work as a concrete task list; what has been done and where — files,
branches, commits; key decisions and why; exact paths/line numbers instead of "see above") to
`.sandcastle/logs/handoff-<task-id>.md`, **commit it on this branch** (use `git add -f` —
`.sandcastle/.gitignore` ignores `logs/`, and the sandbox is destroyed when the run ends, so an
uncommitted note is lost), and end your turn so a fresh agent continues. Small, resumable units
beat one degraded run.
