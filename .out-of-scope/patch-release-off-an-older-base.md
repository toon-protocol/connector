# A patch release off an older base

This repository does not cut a release from anything but `main`. There is no patch line: no
branch off an earlier release that collects cherry-picked fixes, and no handle that names one. A
fix reaches an operator by that operator moving forward to a release that already contains it.

## Why this is out of scope

The decision was made on 2026-11-06 (#1665).

The request was for a release of `2026.09.29.1` plus #1547 (EVM writes bid market fees) and
nothing else. A third-party operator needed the fee fix but couldn't take `2026.11.02.1`, because
that release refuses vouchers with no `network` (ADR 0086), which older payers still send, and a
rollback from it doesn't boot (#1661). The cherry-pick applies cleanly, and the operator was
already running that exact build, made outside this repository.

It is refused for three reasons.

- **Being stuck on an old release is an upgrade-path bug, and it gets fixed on `main`.** If an
  operator can't move forward, then the release that stops them is what needs fixing: it should
  tell them the order to upgrade in, and it should leave a way back. A patch line would hide that
  bug from the next operator instead of fixing it. For #1665 the actual fixes are #1666 (tell the
  operator to upgrade payers first) and #1667 (a rollback after the journal migration).
- **A patch line is a second release process.** It needs a handle scheme that sorts correctly
  against the date handles (a release cut today but holding older code can't be named
  `2026.11.06.N`), a branch to live on, and its own gate run (`cargo test --workspace`,
  `make local-verify`) against that base. All of that would exist for one operator and one
  commit, and every later fix would raise the question of which lines get it too.
- **An operator can already build its own.** The connector is MIT-licensed and the build is
  `cargo build`. An operator who needs a fix before they can upgrade can cherry-pick it and
  build it themselves, as the operator behind #1665 did. That build is theirs to run, and this
  repository doesn't publish it.

`release-connector.yml` enforces this: it refuses a dispatch on any commit that isn't an
ancestor of `origin/main`, and names the release by the UTC date it was cut.

## Prior requests

- #1665: "There is no way to release #1547 alone onto 2026.09.29.1: a release must be on main and named by today's date"
