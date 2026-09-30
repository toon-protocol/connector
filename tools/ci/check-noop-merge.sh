#!/usr/bin/env bash
# No-op merge guard: a PR whose merge result changes zero files must not merge green.
#
# connector#1008 merged as an EMPTY commit: a second PR from a branch whose content
# another PR had already landed. GitHub's Files-changed tab is the three-dot diff
# (the branch's own commits), so the PR looked normal; the MERGE RESULT was empty.
# A `pull_request` run checks out refs/pull/N/merge, whose first parent is the base
# tip and second the PR head, so `git diff <base> HEAD` is exactly what a squash
# would carry. Empty diff, empty merge.
#
# Where it cannot tell (not a PR event, no merge commit because the PR conflicts, a
# stale merge ref) it warns and passes: a required check that fails on "I could not
# tell" is how guards get disabled. It is a job in ci.yml, feeding `CI Status
# Summary`, the one required check -- not a required context of its own, which would
# wedge any PR whose run never reports.
#
# Env: GITHUB_EVENT_NAME, PR_HEAD_SHA, PR_BASE_REF, PR_NUMBER, PR_CHANGED_FILES
#      (the Files-changed count from the event payload; deliberately not recomputed,
#      since `git diff base...head` needs a merge base a shallow clone lacks),
#      GITHUB_STEP_SUMMARY (optional).
# `--self-test` builds throwaway repos and checks the verdict for each shape.
set -uo pipefail

if [ "${1:-}" = "--self-test" ]; then
  self=$(cd "$(dirname "$0")" && pwd)/$(basename "$0")
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  cd "$tmp"
  git init -q -b main repo && cd repo
  git config user.email t@t && git config user.name t
  echo base >f && git add f && git commit -qm base
  fail=0

  # run_guard <name> <expected-exit> <expected-text>: the merge ref is HEAD.
  run_guard() {
    out=$(GITHUB_EVENT_NAME=pull_request PR_HEAD_SHA="$HEAD_SHA" PR_BASE_REF=main \
      PR_NUMBER=1 PR_CHANGED_FILES="$FILES" GITHUB_STEP_SUMMARY=/dev/null bash "$self" 2>&1)
    code=$?
    if [ "$code" != "$2" ] || ! grep -q -- "$3" <<<"$out"; then
      echo "FAIL $1: exit $code, output: $out"; fail=1
    else
      echo "ok   $1"
    fi
  }

  # Real diff: a merge commit of a branch that adds a file.
  git checkout -q -b real && echo x >g && git add g && git commit -qm real
  HEAD_SHA=$(git rev-parse HEAD); git checkout -q main
  git merge -q --no-ff -m merge real
  FILES=1 run_guard "real diff passes" 0 "changes 1 file"
  git reset -q --hard main~1

  # Empty merge, non-empty three-dot diff (#1008): main already has the content.
  git merge -q --no-ff -m "already landed" real   # main now carries g
  git checkout -q -b dup real
  git commit -q --allow-empty -m dup
  HEAD_SHA=$(git rev-parse HEAD); git checkout -q main
  git merge -q --no-ff -m merge dup
  FILES=1 run_guard "duplicate content fails" 1 "already on main"

  # Empty merge, empty three-dot diff: commits that cancel out.
  git reset -q --hard main~1
  git checkout -q -b cancel && echo y >h && git add h && git commit -qm add \
    && git rm -q h && git commit -qm revert
  HEAD_SHA=$(git rev-parse HEAD); git checkout -q main
  git merge -q --no-ff -m merge cancel
  FILES=0 run_guard "cancelling commits fail" 1 "cancel out"

  # Cannot tell: no merge commit, then not a PR event.
  git checkout -q --detach "$HEAD_SHA"
  FILES=0 run_guard "no merge ref warns and passes" 0 "no merge ref"
  out=$(GITHUB_EVENT_NAME=push bash "$self" 2>&1) && echo "ok   push passes" \
    || { echo "FAIL push: $out"; fail=1; }
  exit "$fail"
fi

if [ "${GITHUB_EVENT_NAME:-}" != "pull_request" ]; then
  echo "event is '${GITHUB_EVENT_NAME:-}', not 'pull_request' — no merge result to evaluate"
  exit 0
fi

if ! git rev-parse --verify -q HEAD^2 >/dev/null; then
  echo "::warning::no merge ref for this PR (HEAD is not a merge commit) — the merge result could not be evaluated. A conflicted PR is the usual cause; resolve the conflict and this guard re-runs."
  exit 0
fi

P1=$(git rev-parse HEAD^1)
P2=$(git rev-parse HEAD^2)

# Parent order is base-first, head-second. Assert it against the event payload
# rather than trusting it, so a changed convention degrades to a warning.
if [ "$P2" = "$PR_HEAD_SHA" ]; then
  BASE="$P1"
elif [ "$P1" = "$PR_HEAD_SHA" ]; then
  BASE="$P2"
else
  echo "::warning::neither parent of the merge ref is the PR head ($PR_HEAD_SHA) — the merge ref is probably stale. Not evaluated; push to the branch to refresh it."
  exit 0
fi

if ! git diff --quiet "$BASE" HEAD; then
  CHANGED=$(git diff --name-only "$BASE" HEAD | wc -l)
  echo "✓ merging this PR changes $CHANGED file(s) against $PR_BASE_REF"
  exit 0
fi

BRANCH_FILES="${PR_CHANGED_FILES:-0}"

{
  echo "## ❌ Merging this PR would change nothing"
  echo ""
  echo "The merge result is byte-identical to \`$PR_BASE_REF\`, so squashing this PR"
  echo "would land an **empty commit** — a green check, a closed ticket, and no change."
  echo ""
  if [ "$BRANCH_FILES" -gt 0 ]; then
    echo "**The content is already on \`$PR_BASE_REF\`.** The Files-changed tab still shows"
    echo "$BRANCH_FILES file(s) because that is the three-dot diff against this branch's fork"
    echo "point, not against the branch tip — another PR has landed this same content since."
    echo ""
    echo "Almost certainly a duplicate PR. What to do:"
    echo ""
    echo "1. Find the PR that actually landed it: \`git log --oneline $PR_BASE_REF -- <a file this PR touches>\`."
    echo "2. **Close this PR** and point its ticket at that one."
    echo "3. If the change you meant to make is still missing from \`$PR_BASE_REF\`, it was"
    echo "   never merged by that PR either — diff the file on \`$PR_BASE_REF\` and open a"
    echo "   PR from a branch cut fresh off \`$PR_BASE_REF\`."
  else
    echo "**This branch's own commits cancel out.** Its three-dot diff against"
    echo "\`$PR_BASE_REF\` is empty too, so there was never anything to merge — usually a"
    echo "change and its revert on the same branch, or a branch cut after the work landed."
    echo ""
    echo "Close this PR, or push the change it was supposed to carry."
  fi
  echo ""
  echo "This guard exists because connector#1008 did exactly this and nobody noticed:"
  echo "it merged green, closed its ticket, and \`git show\` returned zero files."
} >>"${GITHUB_STEP_SUMMARY:-/dev/null}"

if [ "$BRANCH_FILES" -gt 0 ]; then
  echo "::error::merging PR #$PR_NUMBER would produce an EMPTY commit — its content is already on $PR_BASE_REF (the Files-changed tab shows $BRANCH_FILES file(s) only because that is the three-dot diff). This is probably a duplicate PR; close it. See the job summary."
else
  echo "::error::merging PR #$PR_NUMBER would produce an EMPTY commit — this branch's commits cancel out, so it has nothing to merge. See the job summary."
fi
exit 1
