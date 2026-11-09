/mattpocock-skills:implement {{ISSUE_URL}}

You are running AFK in a sandbox, on branch `{{BRANCH}}`, which is already checked out.
Nobody will answer a question, so do not ask one. Treat the issue, its comments and its
parent spec (if it has one) as settled. Read them with `gh issue view {{ISSUE_NUMBER}} --comments`.

Commit to `{{BRANCH}}`, and reference `#{{ISSUE_NUMBER}}` in each commit message. Do not
push, open a PR or close the issue. The runner does all three once you finish.

## This repository

- `CLAUDE.md` covers how to run things. `docs/adr/README.md` groups the ADRs by scope, and
  an ADR settles most questions that look like they need a human. ADR 0021 is the
  tiebreaker: the committed vectors are normative and prose is not.
- Line numbers cited in older issues drift. Check that a `file.rs:123` reference still
  points at what the text claims before relying on it.
- `anvil`, `cast` and `solana-test-validator` are installed, so the chain tests really
  run. A chain test that reports `finished in 0.00s` skipped. Treat that as a failure.
- After you finish, the runner runs CI's gate itself and won't open a PR while it is red:
  `cargo fmt --all -- --check`, `cargo build --workspace`, `cargo test --workspace` and
  `cargo clippy --workspace --all-targets -- -D warnings`, plus the npm gate if you
  touched `packages/`. Run them yourself before you finish (see "Ending your session" for
  the order of committing and testing). Never weaken, skip or
  `#[ignore]` a test, and never loosen a lint, to get green.
- A ticket that needs a live box, a funded key or an on-chain write doesn't need a human.
  `.github/workflows/fleet-ops.yml` and `.github/workflows/funded-ops.yml` do that work.
  Dispatch them with `gh workflow run`, run the default `apply: false` first, and quote the
  dry run in a commit message.

## When you cannot finish

Stop only when a genuinely new decision is needed and no ADR covers it, the action is
irreversible, it touches mainnet or real funds, or it needs a credential that no workflow
exposes. In that case, commit nothing and explain what blocks you in a comment on the issue
(`gh issue comment {{ISSUE_NUMBER}}`). The runner moves an issue with no commits to
`needs-triage`.

If your context is getting full (around 150k tokens) before you are done, commit what works,
write the remaining steps to `.sandcastle/logs/handoff-{{ISSUE_NUMBER}}.md`, commit it with
`git add -f`, and end your turn. A fresh session continues from your commits.

## Ending your session

- Write a completion signal only as the very last line of your final message. Never quote,
  paraphrase-with-tags or mention it in a progress note: the runner treats the string
  appearing anywhere in your output as the signal.
- Commit working changes before running the full `cargo test --workspace`, then amend or
  add commits afterwards. The runner starts another session if you leave work uncommitted.
- Never end your turn while a background command is still running. Run the gate in the
  foreground with a timeout. In headless mode, ending the turn ends the session.
- Never use `pkill -f` / `pgrep -f` with a pattern that also appears in your own command
  line, because it matches your own shell.

When the ticket is done and committed, output <promise>COMPLETE</promise>.

If you stopped because you're blocked, output <promise>BLOCKED</promise> instead, after your
comment on the issue. The runner then ends the run. Otherwise it starts another session, which
hits the same blocker and posts the same comment again.
