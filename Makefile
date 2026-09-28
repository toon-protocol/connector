# Development workflow commands for Connector
# Run 'make help' to see all available commands

.PHONY: help build test lint local-build local-preflight local-up local-down local-logs local-rehearse local-verify rust-build rust-test anvil-up anvil-down anvil-logs solana-up solana-down solana-logs solana-mint-usdc infra-up infra-down

# Exported so every `docker compose` this Makefile runs picks it up. The
# anvil service used to bind-mount ./packages/contracts READ-WRITE, which is
# what this pair originally kept host-owned; that package, and the git
# submodules it vendored, left the repository in issue #1386, and the anvil
# service now mounts its inputs read-only (infra/anvil, and
# crates/connector-settlement-evm/contracts for the committed x402 bytecode).
export HOST_UID := $(shell id -u)
export HOST_GID := $(shell id -g)

# Default target - show help
help:
	@echo "Connector Development Commands"
	@echo "=============================="
	@echo ""
	@echo "Build:"
	@echo "  make rust-build           Build the Rust connector workspace"
	@echo "  make build                Build the npm workspaces (devnet faucet tooling)"
	@echo ""
	@echo "Testing:"
	@echo "  make rust-test            Run the Rust workspace tests"
	@echo "  make test                 Run the npm workspace tests"
	@echo "  make lint                 Run linter"
	@echo ""
	@echo "Local Blockchain (EVM):"
	@echo "  make anvil-up             Start Anvil + Faucet (docker compose --profile evm)"
	@echo "  make anvil-down           Stop Anvil + Faucet"
	@echo "  make anvil-logs           Follow EVM docker compose logs"
	@echo ""
	@echo "Local Blockchain (Solana):"
	@echo "  make solana-up            Start Solana test validator (docker compose --profile solana)"
	@echo "  make solana-down          Stop Solana validator"
	@echo "  make solana-logs          Follow Solana docker compose logs"
	@echo "  make solana-mint-usdc     Re-seed the mock-USDC mint (auto-run by solana-up/infra-up)"
	@echo ""
	@echo ""
	@echo "App behind connector: composition lives in the app repos"
	@echo "  (relay/store deploy/docker-compose.yml = connector + that app)."
	@echo ""
	@echo "Local Blockchain (All Chains):"
	@echo "  make infra-up             Start every chain the connector settles on (EVM + Solana)"
	@echo "  make infra-down           Stop them (volumes preserved)"
	@echo ""
	@echo "Local topologies (the shipped image against real chains):"
	@echo "  make local-up             Build the image, start the chains, provision keys, run it"
	@echo "  make local-rehearse       Send a real packet through it; non-zero unless fulfilled"
	@echo "  make local-verify         up + rehearse + down, as CI runs it"
	@echo "  make local-down           Stop it, and remove the state volumes with it"
	@echo "  make local-logs           Follow its logs"
	@echo "  make local-preflight      Ask whether this machine's one stack is free"
	@echo "  LOCAL_TOPOLOGY=<name>     Which topology: solo (default), two-hop, mixed-chain,"
	@echo "                            dealing (one hop converts at a declared rate),"
	@echo "                            onion (a real onion daemon; not on the CI gate)"
	@echo ""
	@echo "Maintenance:"

# Build the Rust connector workspace — the connector itself (ADR 0017).
rust-build:
	cargo build --workspace

# Run the Rust workspace tests, matching ci.yml's Rust Workspace Gate.
rust-test:
	cargo test --workspace

# Build the surviving npm workspaces (devnet faucet tooling).
build:
	npm run build

# Run the surviving npm workspace tests.
test:
	npm test

# NOTE: "app behind the connector" composition lives in the APP repos
# (relay/store `deploy/docker-compose.yml` = connector + that app). The connector
# repo builds only the connector image.

# Run linter
lint:
	npm run lint

# Local Blockchain — EVM (Anvil + Faucet)
anvil-up:
	docker compose --profile evm up -d

anvil-down:
	docker compose --profile evm down

anvil-logs:
	docker compose --profile evm logs -f

# Local Blockchain — Solana (Test Validator)
# The validator loads `payment-channels` and p-token into GENESIS from the
# committed fixtures (infra/solana/entrypoint.sh's --bpf-program), so nothing
# is built first: TOON's own program is no longer loaded (ADR 0075, #1385).
solana-up:
	docker compose --profile solana up -d
	$(MAKE) solana-mint-usdc

solana-down:
	docker compose --profile solana down

solana-logs:
	docker compose --profile solana logs -f

# Re-seed the deterministic mock-USDC mint after a validator (re)create. The
# validator entrypoint runs `solana-test-validator --reset` on every start
# (see infra/solana/entrypoint.sh), which wipes the mint along with the rest
# of the chain state -- previously this required manually re-running
# infra/solana/create-usdc-mint.sh on the host, so every faucet USDC drip
# failed with TokenAccountNotFoundError until someone noticed (issue #351).
# The script is idempotent: it skips creation if the mint exists and always
# tops up the treasury.
#
# This FAILS the target when the mint cannot be seeded. It used to end in
# `|| echo "WARNING: ..."`, which is the silent-skip ADR 0007 bans in the same
# words it bans a chain-less test reporting `passed`: a `solana-up` that
# prints a warning and exits 0 leaves a validator with no USDC mint, and the
# committed `token_address` in every local connector config then names an
# account that does not exist. That surfaces later as an opaque settlement
# failure instead of here, as a missing CLI.
#
# It runs on the HOST because the beeman validator image ships no `spl-token`.
# That is the one non-container dependency in the local stack; see the script's
# own header.
solana-mint-usdc:
	@echo "Waiting for Solana validator to be ready..."
	@for i in $$(seq 1 60); do \
		docker compose --profile solana exec -T solana-validator curl -sf http://localhost:8899/health 2>/dev/null | grep -q ok && break; \
		sleep 2; \
	done
	@command -v spl-token >/dev/null 2>&1 || { \
		echo "ERROR: spl-token is not on PATH. The mock-USDC mint cannot be seeded, and a"; \
		echo "       validator without it cannot settle -- refusing to report success."; \
		echo "       Install the SPL token CLI: cargo install spl-token-cli"; \
		exit 1; \
	}
	@command -v solana >/dev/null 2>&1 || { \
		echo "ERROR: solana is not on PATH. Install the Solana CLI: https://solana.com/docs/intro/installation"; \
		exit 1; \
	}
	./infra/solana/create-usdc-mint.sh http://localhost:8899

# Local Blockchain — every chain the Rust connector actually settles on.
#
# There is no Mina profile: ADR 0065 removed Mina from this repository outright
# (ADR 0002 had already dropped it from the connector).
#
# infra-down intentionally does NOT pass -v (preserves existing per-profile volumes).
infra-up:
	docker compose --profile evm --profile solana up -d
	$(MAKE) solana-mint-usdc

infra-down:
	docker compose --profile evm --profile solana down

# ─────────────────────────────────────────────────────────────────────────────
# Local topologies (local/) -- the SHIPPED IMAGE, run against real containerised
# chains. Not a substitute for `make rust-test`, which covers the connector's
# behaviour far better by spawning its own chains per test (ADR 0007). This
# covers the one thing that cannot: that the image boots on a mounted config
# and serves a packet.
# ─────────────────────────────────────────────────────────────────────────────
LOCAL_TOPOLOGY ?= solo
LOCAL_COMPOSE := docker compose -f docker-compose.yml -f local/$(LOCAL_TOPOLOGY)/compose.yml \
	--profile evm --profile solana --profile $(LOCAL_TOPOLOGY)

# The compose project every one of these targets works in. Declared in
# docker-compose.yml as `name: connector` -- written out again here only so
# `local-down` can address the project WITHOUT a compose file, which is what
# lets it reach a stack some other topology started. The two are held to one
# figure by `local_topologies_load.rs::every_local_target_names_one_compose_project`.
#
# It used to follow the directory, and that is issue #1122: a stack started
# from a git worktree was a different project from the same repository's main
# checkout, so `make local-down` in one could not see the other's containers,
# network or -- the part that matters -- its state volumes. A `connector_solo-state`
# survived two days on this machine that way, and a state volume outliving a run
# is exactly what makes the next rehearsal's money assertion vacuous.
LOCAL_PROJECT := connector

# The connector services each topology runs, listed rather than discovered.
# `up -d --wait` with no arguments would start every service in the enabled
# profiles -- which includes the `faucet`, an app-layer service local/ has no
# business running (local/README.md, "Connector layer only"). Naming them keeps
# that decision visible instead of leaving it to a profile's membership.
LOCAL_NODES_solo := connector
LOCAL_NODES_two-hop := connector-a connector-b
LOCAL_NODES_mixed-chain := connector-a connector-b connector-c
# `onion` names its two connectors and NOT its two `anon` sidecars, and that is
# not an omission: local/keys.sh starts those itself, before this list is used,
# because the hidden-service address they generate has to be rendered into the
# configs these two are about to mount (ADR 0070 decision 7). They are still waited on
# -- each connector `depends_on` its own daemon's health gate.
LOCAL_NODES_onion := connector-a connector-b
LOCAL_NODES_dealing := connector-a connector-b connector-c
LOCAL_NODES = $(LOCAL_NODES_$(LOCAL_TOPOLOGY))

# The image the topologies run. Built from this working tree, deliberately: the
# question is whether THIS commit's image boots, and pulling a published tag
# would answer it about some other commit.
local-build:
	docker build -f deploy/connector-rust/Dockerfile -t connector-rust:local .

# Chains first, then keys (they need a chain to be funded ON), then the
# connector (it needs the key files to exist before it will start). That order
# is why this is not one `up`.
# Both `--wait`s below are load-bearing rather than tidy.
#
# On the chains: anvil's health gate is "the last contract infra/anvil/seed.sh
# creates has code", so waiting is what makes the seed complete before keys.sh
# mints against it. Without it `up -d` returns as soon as the containers start,
# and every step after races the seed -- a `cast send` of `mint(...)` to a
# codeless address does not revert, so the funding silently does nothing.
#
# On the connectors: this target's contract is that when it returns, the
# topology can be SENT TO. Their health gate is a real request to the client
# edge, so returning before that passes hands `local-rehearse` a connector that
# is merely "Started" -- the distinction ADR 0041 had to learn for the fleet:
# the container being Up is not sufficient evidence. In a multi-node topology
# each node also waits on the one it dials, so `--wait` here means every hop on
# the path is serving, not just the one the packet is handed to.
#
# And that is why keys.sh runs TWICE. Every local channel is an x402 channel,
# a peering is two of them, and each is opened by its own payer's RUNNING node
# (ADR 0075): the second call sends each end of every peering a signed
# `POST /peers`, tops the payer's channel up with `POST /channels/:id/fund`,
# writes the forwarding routes with `POST /routes/peers`, and reads every
# channel back off its chain, failing this target if the chain disagrees. It
# is a no-op on `solo`, which has no peering.
local-up: local-preflight local-build
	@test -n "$(LOCAL_NODES)" || { \
		echo "ERROR: LOCAL_TOPOLOGY='$(LOCAL_TOPOLOGY)' has no LOCAL_NODES_ entry in this Makefile."; \
		echo "       Known topologies: solo two-hop mixed-chain onion dealing."; \
		exit 1; \
	}
	@$(LOCAL_COMPOSE) up -d --wait anvil solana-validator || { \
		echo ""; \
		echo "ERROR: the chains did not come up. Compose's own message is above:"; \
		echo "         'address already in use'  -- something else already holds 8545 or"; \
		echo "                                      8899. 'ss -tlnp | grep 8545' names it."; \
		echo "         'is unhealthy'            -- the container started but never passed"; \
		echo "                                      its gate. anvil's gate is 'the seed's"; \
		echo "                                      last contract has code', so an"; \
		echo "                                      unhealthy anvil is a failed seed."; \
		echo "       Whatever did start is still running; 'make local-down' clears it."; \
		anvil_log=$$($(LOCAL_COMPOSE) logs --no-color --no-log-prefix anvil 2>/dev/null \
			| grep -vE '^(eth_|net_|web3_|anvil_|debug_|trace_|txpool_)' | tail -40); \
		if [ -n "$$anvil_log" ]; then \
			echo ""; \
			echo "--- anvil's log, with the per-request RPC noise stripped ---"; \
			echo "$$anvil_log"; \
		fi; \
		exit 1; \
	}
	$(MAKE) solana-mint-usdc
	cargo build --release -p connector
	./local/keys.sh $(LOCAL_TOPOLOGY)
	$(LOCAL_COMPOSE) up -d --wait $(LOCAL_NODES)
	./local/keys.sh $(LOCAL_TOPOLOGY) channels

# Is this machine's one local stack free for this topology to take? Run as the
# FIRST prerequisite of `local-up`, and separately of `local-verify`, so a
# refusal costs nothing and -- more to the point -- so `local-verify` never
# reaches its own failure path. That path ends in `local-down`, and tearing
# down the stack this guard just refused to disturb would destroy the very
# thing it protected: another checkout's containers and state volumes.
#
# Callable by hand too, to ask "is anything up, and whose?".
local-preflight:
	@./local/stack-guard.sh $(LOCAL_PROJECT) $(LOCAL_TOPOLOGY) $(CURDIR)

# `-v`, and that matters. The named volumes here hold the connectors' claim
# journals, and both local chains wipe their own state on every start -- so
# keeping the journals across a down/up pairs a live watermark with a chain
# that no longer has the history behind it. Concretely it also makes the
# rehearsal's money assertion vacuous: a peered topology's sender proves the
# peering was paid by reading the payee's journal, and a journal left behind by
# the LAST run satisfies that read without this run having paid anything.
#
# So it has to remove the volumes of whatever is actually up, not of whatever
# LOCAL_TOPOLOGY happens to say -- and until the project name was fixed it
# could not even do that much, because a stack started from another directory
# was a different project and this target could not see it at all (#1122).
# Both halves of the fix are here: `--remove-orphans` sweeps containers this
# topology's files do not declare, and the volume pass afterwards is by PROJECT
# LABEL rather than by anything the loaded files mention, so a `two-hop-b-state`
# left by another topology is removed by a `LOCAL_TOPOLOGY=solo` teardown.
local-down:
	$(LOCAL_COMPOSE) down -v --remove-orphans
	@stale=$$(docker volume ls -q --filter label=com.docker.compose.project=$(LOCAL_PROJECT)); \
	if [ -n "$$stale" ]; then \
		echo "Removing state volumes left by another topology of this project:"; \
		echo "$$stale" | sed 's/^/  /'; \
		echo "$$stale" | xargs docker volume rm; \
	fi

local-logs:
	$(LOCAL_COMPOSE) logs -f

# The assertion. `connector send --expect-fulfill` exits non-zero on anything
# that is not a correctly-fulfilled packet, so this target's exit status is the
# verdict -- there is no output to grep and nothing that can pass by printing.
#
# A topology with a peering asks its `sender` for a second thing, because
# `--expect-fulfill` structurally cannot cover it: a peer claim's verdict rides
# back in `Toon-Claim-Ack` and never gates the packet, so a peering whose every
# claim was refused still fulfils. Those senders read the payee's own claim
# journal afterwards and exit non-zero if the crossing was carried for free.
local-rehearse:
	$(LOCAL_COMPOSE) --profile sender run --rm sender

# Bring it up, prove it, tear it down. What CI runs.
#
# The logs are dumped HERE rather than in a workflow step, because by the time
# a workflow step runs the containers are already gone -- `local-down` removes
# them, and it has to run on the failure path too or CI leaks a stack.
#
# BOTH failure paths, which is the fix for a gap this target shipped with:
# `local-up` was a plain prerequisite line, so a bring-up that failed aborted
# the recipe before either the log dump or `local-down` could run. The stack
# was left standing -- containers, network and named volumes -- and the entire
# diagnosis a developer got was compose's own `container ... is unhealthy`.
# Measured, not reasoned about: an anvil whose contract deploy had failed hung
# `local-up` for three minutes and then leaked two running chains.
#
# The dump drops two per-request noise streams -- the validator's ~30/sec slot
# line and anvil's one line per RPC method -- because `--tail` is applied PER
# SERVICE and those two fill their whole allowance with nothing. Filtering them
# cannot hide a failure: the exit status is the verdict here, not the log.
#
# `local-preflight` is a PREREQUISITE rather than the first line of the recipe,
# and that placement is the point: everything below ends in `local-down`, so a
# refusal that arrived inside the recipe would tear down the other checkout's
# stack the guard had just declined to touch.
local-verify: local-preflight
	@$(MAKE) local-up; status=$$?; \
	if [ $$status -eq 0 ]; then \
		$(MAKE) local-rehearse; status=$$?; \
		if [ $$status -ne 0 ]; then \
			echo "=== rehearsal FAILED (exit $$status) -- logs follow ==="; \
		fi; \
	else \
		echo "=== bring-up FAILED (exit $$status) -- logs follow ==="; \
	fi; \
	if [ $$status -ne 0 ]; then \
		$(LOCAL_COMPOSE) logs --no-color --tail=200 2>/dev/null \
			| grep -vE '\| *(Processed Slot:|eth_|net_|web3_|anvil_|debug_|trace_|txpool_)' \
			|| true; \
	fi; \
	$(MAKE) local-down; \
	exit $$status

