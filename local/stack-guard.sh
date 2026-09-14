#!/usr/bin/env bash
#
# Refuse to bring a topology up on top of somebody else's stack.
#
# `docker-compose.yml` names the compose project `connector` (see its header),
# so every checkout of this repository on a machine now shares one project
# rather than one per directory. That is the point -- `make local-down` reaches
# whatever `make local-up` created, from wherever it is run -- but it means the
# second `up` no longer gets a private stack it can silently half-start. It
# would ADOPT the first one: compose matches services by name within a project,
# so the anvil already running for another checkout is reused with that
# checkout's packages/contracts bind-mounted into it, and a topology's own
# connectors come up beside a different topology's, each still holding the
# ports and the state volumes the other cannot see.
#
# One stack per machine is what the machine can hold anyway -- every topology
# publishes 8545, 8899 and its connectors' client edges -- so the collision is
# not a capability being taken away. What this adds is that it is REPORTED,
# naming the directory and the topology that already hold the stack, instead of
# arriving as a port bind failure inside a container the other checkout's
# `make local-down` was never going to remove.
#
# Usage: local/stack-guard.sh <project> <topology> <working-dir>
set -euo pipefail

project=${1:?usage: stack-guard.sh <project> <topology> <working-dir>}
topology=${2:?usage: stack-guard.sh <project> <topology> <working-dir>}
here=${3:?usage: stack-guard.sh <project> <topology> <working-dir>}

if ! command -v docker >/dev/null 2>&1; then
  echo "ERROR: docker is not on PATH. The local topologies ARE containers; there is"
  echo "       nothing here that can run without it."
  exit 1
fi

# Stopped containers count. They still own the project's names and still hold
# the state volumes, and `make local-down` is the thing that clears both.
owners=$(docker ps -a \
  --filter "label=com.docker.compose.project=$project" \
  --format '{{.Label "com.docker.compose.project.working_dir"}}	{{.Label "com.docker.compose.project.config_files"}}' |
  sort -u)

if [ -n "$owners" ]; then
  owner_dir=$(head -1 <<<"$owners" | cut -f1)
  owner_files=$(cut -f2 <<<"$owners" | tr ',' '\n' | sort -u | tr '\n' ' ')

  if [ "$owner_dir" != "$here" ]; then
    echo "ERROR: a '$project' compose stack is already up, started from another directory:"
    echo "         $owner_dir"
    echo "       This one is $here."
    echo "       Both would be the same compose project, so this run would adopt those"
    echo "       containers -- including an anvil with the OTHER checkout's"
    echo "       packages/contracts mounted into it."
    echo ""
    echo "       'make local-down' now reaches that stack from here (the project name no"
    echo "       longer follows the directory). Run it, then try again."
    exit 1
  fi

  # A topology already up. Its connectors are different services, so compose
  # would leave them running beside this one's, both bound to the same chains
  # and each with its own state volumes -- and the money assertion of whichever
  # one is rehearsed would be read out of a journal the other is also writing.
  other=$(grep -oE 'local/(solo|two-hop|mixed-chain|onion|dealing)/compose\.yml' <<<"$owner_files" |
    sed -E 's|local/(.*)/compose\.yml|\1|' | sort -u | grep -v "^${topology}$" || true)
  if [ -n "$other" ]; then
    echo "ERROR: the '$other' topology is already up in this directory, and every topology"
    echo "       here is the same compose project on the same chain ports."
    echo "       Run 'make local-down' first -- it removes the containers AND the state"
    echo "       volumes, which is what keeps the next rehearsal's money assertion honest"
    echo "       (local/README.md, 'What --expect-fulfill cannot see')."
    exit 1
  fi

  exit 0
fi

# No containers, but the project's named volumes are still there: a run that
# was killed rather than torn down. Those volumes are the connectors' claim
# journals, and local/README.md is explicit about why they must not outlive a
# run -- a journal left by the last run satisfies this run's money check
# without this run having paid anything, while both chains have meanwhile
# wiped the history behind it.
stale=$(docker volume ls -q --filter "label=com.docker.compose.project=$project")
if [ -n "$stale" ]; then
  echo "ERROR: no '$project' containers are running, but its state volumes are still here:"
  sed 's/^/         /' <<<"$stale"
  echo "       They hold the connectors' claim journals from a run that was killed rather"
  echo "       than torn down, and both local chains have wiped the history behind them."
  echo "       Reusing one makes the rehearsal's money assertion vacuous."
  echo ""
  echo "       'make local-down' removes them."
  exit 1
fi
