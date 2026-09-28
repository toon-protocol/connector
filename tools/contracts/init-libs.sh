#!/usr/bin/env bash
#
# Put packages/contracts/lib at the revisions this repository pins, from the
# host, before any container compiles against it.
#
# `packages/contracts` builds against two git submodules -- forge-std and
# openzeppelin-contracts -- and until this script existed the only thing that
# ever installed them on a checkout made without `--recursive` was the compose
# `anvil` service's `forge install` self-heal. That install named no revision,
# so it took each repository's DEFAULT BRANCH: OpenZeppelin 5.7.0 into a tree
# whose submodule says fcbae539 (5.5.0), measured on a worktree of cec95059
# (issue #1121). The drift is not confined to the container, because the mount
# is the developer's own source tree: the next host-side `forge build` compiles
# against it too, and that includes
# `crates/connector-settlement-evm/tests/abi_provenance.rs`, which diffs the
# committed ABI against a fresh build. An upstream release touching anything
# the ABI depends on turns that gate red on a machine that changed nothing --
# and it reads as "someone edited a contract".
#
# So the pin is applied from the one place that HOLDS the pin: a git checkout,
# where the submodule sha is a committed fact rather than a string somebody
# retyped. (The anvil container no longer compiles these contracts at all: it
# places committed x402 bytecode, `infra/anvil/seed.sh`, ADR 0075 -- so the
# host-side `forge build` is the only consumer left, until #1386 removes the
# package.)
#
# This is a self-heal, not a gate. It is silent and cheap when the tree is
# already correct (~20ms), and it declines rather than fails when there is no
# git checkout to read a pin out of -- a source tarball has no submodule shas.
set -euo pipefail

# The directory prefix this script is allowed to touch. Everything below --
# including the one `rm -rf` -- is filtered through it, so a `.gitmodules` that
# grows a submodule somewhere else cannot be reached from here.
readonly LIB_PREFIX="packages/contracts/lib/"

if ! command -v git >/dev/null 2>&1; then
  echo "note: git is not on PATH, so $LIB_PREFIX cannot be pinned from the host."
  exit 0
fi

repo_root=$(git rev-parse --show-toplevel 2>/dev/null || true)
if [ -z "$repo_root" ]; then
  echo "note: not a git checkout, so $LIB_PREFIX cannot be pinned from the host."
  exit 0
fi
cd "$repo_root"

# The submodule paths under packages/contracts/lib, read from .gitmodules
# rather than listed here so a new one is picked up without editing this file.
mapfile -t lib_paths < <(
  git config -f .gitmodules --get-regexp '^submodule\..*\.path$' |
    awk '{ print $2 }' |
    grep "^${LIB_PREFIX}" || true
)

if [ ${#lib_paths[@]} -eq 0 ]; then
  echo "ERROR: .gitmodules names no submodule under $LIB_PREFIX, but"
  echo "       packages/contracts builds against forge-std and openzeppelin-contracts."
  echo "       Either .gitmodules or this script is wrong; refusing to guess."
  exit 1
fi

# A lib directory that is POPULATED while its submodule is still uninitialized
# is not a submodule at all -- it is what an older, unpinned `forge install`
# left behind, and it is the exact state issue #1121 measured on this machine.
# `git submodule update --init` cannot repair it: it tries to clone and dies
# with "destination path ... already exists and is not an empty directory",
# naming neither forge nor the drift nor a way out. So the unregistered content
# is removed first, loudly. It is disposable by construction -- a dependency
# checkout, reinstalled below at the committed revision.
for path in "${lib_paths[@]}"; do
  status_line=$(git submodule status -- "$path" 2>/dev/null || true)
  case "$status_line" in
    -*)
      if [ -n "$(ls -A "$path" 2>/dev/null || true)" ]; then
        echo "$path holds files but is not an initialized submodule -- an unpinned"
        echo "  'forge install' leaves exactly this. Removing it so the pinned revision"
        echo "  can be checked out."
        rm -rf "${repo_root:?}/${path:?}"
      fi
      ;;
  esac
done

git submodule update --init -- "${lib_paths[@]}"

# `git submodule update` reports a failed clone on stderr and can still exit 0
# for the run as a whole, so the state is read back rather than trusted. A
# leading space is "checked out at the sha the index names"; `-` is
# uninitialized and `+` is checked out at some OTHER sha, which is the drift
# this script exists to end.
failed=0
while IFS= read -r line; do
  case "$line" in
    ' '*) continue ;;
  esac
  echo "ERROR: $(awk '{ print $2 }' <<<"$line") is not at the revision this repository"
  echo "       pins. 'git submodule status' says:"
  echo "         $line"
  failed=1
done < <(git submodule status -- "${lib_paths[@]}")

if [ "$failed" -ne 0 ]; then
  echo "       packages/contracts would compile against something other than the"
  echo "       committed pin, which is how a stale OpenZeppelin reaches abi_provenance."
  echo "       Fix the tree ('git submodule update --init ${LIB_PREFIX}') and re-run."
  exit 1
fi
