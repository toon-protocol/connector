# Epic 23: Unified Deployment Infrastructure — Brownfield Enhancement

**Epic Number:** 23
**Priority:** High — Phases 3-6 of UNIFIED-DEPLOYMENT-PLAN.md
**Type:** Infrastructure & Deployment
**Dependencies:** Epic 20 (bidirectional middleware), Epic 21 (payment channel admin APIs), Epic 22 (middleware simplification)

## Epic Goal

Create the unified deployment infrastructure that orchestrates agent-runtime connectors, agent-runtime middleware, and agent-society BLS containers into a single deployable stack. Deliver a 16-service Docker Compose file, K8s manifests for agent-society and agent-runtime-core, an updated deploy script with `--unified` flag, and the environment configuration for Nostr keypairs and settlement contract addresses.

## Epic Description

### Existing System Context

**Current Functionality:**

- `docker-compose-5-peer-multihop.yml` deploys 6 services (TigerBeetle + 5 connector peers) in a linear topology with settlement support
- `docker-compose-5-peer-agent-runtime.yml` deploys 2 services (agent-runtime + mock business logic) for standalone middleware testing
- `docker-compose-5-peer-nostr-spsp.yml` adds Nostr relay containers for SPSP testing
- `scripts/deploy-5-peer-multihop.sh` (49KB, 1500+ lines) supports `--with-agent`, `--with-nostr-spsp`, `--agent-only` flags but no `--unified` flag for the full 3-layer stack
- K8s manifests exist for `k8s/agent-runtime/`, `k8s/connector/`, `k8s/tigerbeetle/` — but **not** for `k8s/agent-society/`
- `.env.peers` contains EVM private keys and addresses but no Nostr keypairs, settlement contract addresses, or agent-society configuration

**Technology Stack:**

- Docker Compose v3.8, Kustomize for K8s overlays
- TigerBeetle 0.16.68 (accounting), Anvil (local EVM), agent-runtime (connector), agent-runtime-core (middleware), agent-society (BLS + Nostr relay)
- Agent-society container exposes port 3100 (BLS HTTP) + 7100 (Nostr relay WebSocket)
- Agent-runtime middleware connects to connector BTP and BLS HTTP
- Connector Admin API on port 8081 (internal), BTP on port 3000+N

**Integration Points:**

- `docker-compose-unified.yml` — New file: 16-service orchestration
- `k8s/agent-society/` — New directory: K8s manifests for BLS + Nostr relay
- `k8s/connector/base/configmap.yaml` — Add `LOCAL_DELIVERY_ENABLED` and `LOCAL_DELIVERY_URL`
- `scripts/deploy-5-peer-multihop.sh` — Add `--unified` flag with bootstrap verification
- `.env.peers` — Extend with Nostr keypairs and contract addresses

### Enhancement Details

**What's Being Added:**

1. **Unified Docker Compose** (`docker-compose-unified.yml`): 16 services on a shared `agent-network` bridge:
   - 1x TigerBeetle (accounting DB)
   - 5x connector peers (peer1-peer5) — ILP routing, settlement, payment channels
   - 5x agent-runtime middleware (agent-runtime-1 through agent-runtime-5) — thin bidirectional proxy
   - 5x agent-society containers (agent-society-1 through agent-society-5) — BLS + Nostr relay + bootstrap
   - Startup dependency chain: TigerBeetle → agent-society-{N} (BLS healthy) → agent-runtime-{N} (middleware healthy) → peer{N} (connector healthy)
   - Each agent-society container is wired to its agent-runtime via `AGENT_RUNTIME_URL`, to its connector via `CONNECTOR_ADMIN_URL`, and publishes kind:10032 via its Nostr relay
   - agent-society-1 is the bootstrap node (`SPSP_MIN_PRICE=0`, `KNOWN_PEERS=[]`); peers 2-5 have `KNOWN_PEERS` pointing to agent-society-1

2. **K8s Manifests for agent-society**: Kustomize base + staging/production overlays with:
   - Deployment (not StatefulSet — in-memory SQLite, no persistent state)
   - ConfigMap for non-sensitive config (BLS_PORT, WS_PORT, ASSET_CODE, CONNECTOR_ADMIN_URL)
   - Secret for NOSTR_SECRET_KEY
   - Two Services: ClusterIP for BLS HTTP (connector talks to this), headless for Nostr relay WebSocket (peer discovery)
   - NetworkPolicy, PodDisruptionBudget, ServiceAccount
   - Probes on `/health` (BLS port 3100)

3. **Deploy script `--unified` flag**: Extends `deploy-5-peer-multihop.sh` with:
   - Phase 1: Start TigerBeetle + agent-society containers, wait for BLS + relay health
   - Phase 2: Start agent-runtime middleware, wait for health (including BTP client connected)
   - Phase 3: Start connectors, wait for health + Admin API ready
   - Phase 4: Wait for bootstrap — verify relay discovery, peer registration, 0-amount SPSP handshakes
   - Phase 5: Verify payment channels opened (`GET /admin/channels` on each connector)
   - Phase 6: Verify routing tables populated (`GET /admin/peers`)
   - Phase 7: Send end-to-end test packet (g.peer1 → g.peer5, verify FULFILL)

4. **Environment configuration** (`.env.peers` extension): Nostr keypairs (per-peer), settlement contract addresses (shared), agent-society config

**How It Integrates:**

- Docker Compose wires the 3-layer architecture: connector ↔ agent-runtime ↔ agent-society
- Each connector has `LOCAL_DELIVERY_ENABLED=true` and `LOCAL_DELIVERY_URL=http://agent-runtime-{N}:3100` so inbound ILP packets route to middleware
- Each agent-runtime has `CONNECTOR_BTP_URL=ws://peer{N}:3000` for outbound packet injection and `BUSINESS_LOGIC_URL=http://agent-society-{N}:3100` for BLS calls
- Each agent-society has `CONNECTOR_ADMIN_URL=http://peer{N}:8081` for channel management and `AGENT_RUNTIME_URL=http://agent-runtime-{N}:3100` for outbound ILP sends
- K8s manifests use cross-namespace networking: connector → agent-runtime-core:3100, agent-runtime-core → agent-society:3100, agent-society → connector admin:8081
- Bootstrap flow uses 0-amount ILP packets through the full stack (agent-society → agent-runtime → connector → BTP → peer connector → agent-runtime → agent-society)

**Success Criteria:**

1. `docker compose -f docker-compose-unified.yml up` starts all 16 services with correct dependency chain
2. All services reach healthy state within 120 seconds
3. Bootstrap completes: peers 2-5 discover peer1 via relay, register with connector, perform 0-amount SPSP handshake
4. Payment channels opened between peers (verifiable via `GET /admin/channels`)
5. End-to-end test packet (g.peer1 → g.peer5) returns FULFILL
6. K8s manifests apply cleanly: `kubectl apply -k k8s/agent-society/`
7. Deploy script `--unified` flag runs full verification suite

## Stories

### Story 23.1: Unified Docker Compose (16 Services)

**As a** developer,
**I want** a single Docker Compose file that deploys the complete 3-layer agent network (connector + middleware + BLS),
**so that** I can test the full unified deployment locally with a single command.

**Scope:**

- **Create `docker-compose-unified.yml`** with 16 services:
  - `tigerbeetle`: Same as existing `docker-compose-5-peer-multihop.yml` TigerBeetle config (image `ghcr.io/tigerbeetle/tigerbeetle:0.16.68`, volume `/tmp/m2m-tigerbeetle:/data`, security_opt seccomp=unconfined, cap IPC_LOCK, mem_limit 4g)
  - `agent-society-{1..5}`: Image `agent-society`, ports `311{N-1}:3100` (BLS) + `711{N-1}:7100` (relay), environment from `.env.peers` (NOSTR_SECRET_KEY, ILP_ADDRESS, BTP_ENDPOINT, CONNECTOR_ADMIN_URL, AGENT_RUNTIME_URL, settlement config), healthcheck on `GET /health` port 3100
  - `agent-runtime-{1..5}`: Image `agent-runtime-core`, ports `320{N-1}:3100`, environment (PORT, BASE_ADDRESS, BUSINESS_LOGIC_URL, CONNECTOR_BTP_URL, CONNECTOR_BTP_AUTH_TOKEN), healthcheck on `GET /health` port 3100
  - `peer{1..5}`: Same as existing multihop config but with added `LOCAL_DELIVERY_ENABLED=true`, `LOCAL_DELIVERY_URL=http://agent-runtime-{N}:3100`, `ADMIN_API_ENABLED=true`, ports (BTP 300{N-1}, Health 908{N-1}, Admin 818{N-1})
- **Startup dependency chain:**
  ```
  tigerbeetle (service_started)
    → agent-society-{N} (service_healthy via GET /health)
      → agent-runtime-{N} (service_healthy via GET /health)
        → peer{N} (service_healthy via GET /health)
  ```
- **Environment wiring per peer** (example peer1):
  - peer1 connector: `LOCAL_DELIVERY_URL=http://agent-runtime-1:3100`
  - agent-runtime-1: `BUSINESS_LOGIC_URL=http://agent-society-1:3100`, `CONNECTOR_BTP_URL=ws://peer1:3000`
  - agent-society-1 (bootstrap): `CONNECTOR_ADMIN_URL=http://peer1:8081`, `AGENT_RUNTIME_URL=http://agent-runtime-1:3100`, `SPSP_MIN_PRICE=0`, `KNOWN_PEERS=[]`
  - agent-society-2 (non-bootstrap): `KNOWN_PEERS=[{"pubkey":"${PEER1_NOSTR_PUBKEY}","relayUrl":"ws://agent-society-1:7100","btpEndpoint":"ws://peer1:3000"}]`
- **Network:** Single `agent-network` bridge, all 16 services
- **Update `.env.peers`** with Nostr keypairs (PEER{N}\_NOSTR_SECRET_KEY, PEER{N}\_NOSTR_PUBKEY), settlement contract addresses (AGENT_TOKEN_ADDRESS, TOKEN_NETWORK_ADDRESS), and per-peer settlement addresses (reuse existing PEER{N}\_EVM_ADDRESS)

**Acceptance Criteria:**

1. `docker compose -f docker-compose-unified.yml config` validates without errors
2. All 16 services start with correct dependency ordering
3. agent-society containers healthy before agent-runtime starts
4. agent-runtime containers healthy before connector starts
5. All BTP auth tokens match between connector and agent-runtime pairs
6. Host port mappings don't conflict: BTP 3000-3004, Health 9080-9084, Admin 8181-8185, BLS 3110-3114, Relay 7110-7114, Middleware 3200-3204
7. agent-society-1 configured as bootstrap node (SPSP_MIN_PRICE=0)
8. agent-society-{2..5} have KNOWN_PEERS pointing to agent-society-1
9. `.env.peers` extended with Nostr keypairs and contract addresses (with placeholder values and generation instructions)
10. Settlement environment variables wired: SUPPORTED*CHAINS, SETTLEMENT_ADDRESS*_, PREFERRED*TOKEN*_, TOKEN*NETWORK*\*

---

### Story 23.2: K8s Manifests for Agent-Society and Connector Config Update

**As a** DevOps engineer,
**I want** Kubernetes manifests for the agent-society service and updated connector configuration,
**so that** the full 3-layer stack can be deployed to K8s with Kustomize.

**Scope:**

- **Create `k8s/agent-society/` directory structure:**
  ```
  k8s/agent-society/
  ├── base/
  │   ├── namespace.yaml          # agent-society namespace
  │   ├── serviceaccount.yaml     # ServiceAccount for agent-society pods
  │   ├── configmap.yaml          # Non-sensitive config (BLS_PORT, WS_PORT, ASSET_CODE, ASSET_SCALE, BASE_PRICE_PER_BYTE, CONNECTOR_ADMIN_URL)
  │   ├── secret.yaml             # NOSTR_SECRET_KEY (placeholder, replaced by SealedSecrets/SOPS in prod)
  │   ├── deployment.yaml         # BLS + relay co-located, ports 3100 + 7100
  │   ├── service.yaml            # ClusterIP :3100 (BLS HTTP) + headless :7100 (relay WebSocket)
  │   ├── networkpolicy.yaml      # Ingress from agent-runtime-core namespace on 3100; ingress from agent-society namespace on 7100
  │   ├── pdb.yaml                # PodDisruptionBudget (minAvailable: 1)
  │   └── kustomization.yaml
  ├── overlays/
  │   ├── staging/
  │   │   └── kustomization.yaml  # Staging-specific config patches
  │   └── production/
  │       └── kustomization.yaml  # Production-specific config patches
  └── kustomization.yaml          # Root kustomization pointing to base
  ```
- **Deployment details (base/deployment.yaml):**
  - 1 replica (Deployment, not StatefulSet — in-memory SQLite)
  - Security context: runAsNonRoot (UID 1000), readOnlyRootFilesystem
  - Two ports: `http` (3100) for BLS, `ws` (7100) for relay
  - Resources: requests 128Mi/100m, limits 256Mi/500m (matches agent-runtime pattern)
  - Probes on `/health` port 3100 (startup: 12 retries × 5s, liveness: 15s interval, readiness: 10s interval)
  - Pod anti-affinity (prefer spreading across nodes)
  - envFrom: configMapRef + secretRef
- **Service details (base/service.yaml):**
  - ClusterIP service for BLS HTTP on port 3100 (connector's agent-runtime connects here)
  - Headless service for relay WebSocket on port 7100 (peer-to-peer Nostr relay discovery)
- **NetworkPolicy (base/networkpolicy.yaml):**
  - Ingress on port 3100: from `agent-runtime-core` namespace (middleware → BLS)
  - Ingress on port 7100: from `agent-society` namespace (peer relay → peer relay) and `agent-runtime-core` namespace
  - Egress: to `connector` namespace on port 8081 (BLS → connector Admin API), to `agent-runtime-core` namespace on port 3100 (BLS → middleware /ilp/send)
- **Update `k8s/connector/base/configmap.yaml`:**
  - Add `LOCAL_DELIVERY_ENABLED: 'true'`
  - Add `LOCAL_DELIVERY_URL: 'http://agent-runtime-core.agent-runtime-core.svc.cluster.local:3100'`

**Acceptance Criteria:**

1. `kubectl apply -k k8s/agent-society/base/` applies cleanly (dry-run)
2. `kubectl apply -k k8s/agent-society/overlays/staging/` applies cleanly (dry-run)
3. Deployment creates pod with two container ports (3100, 7100)
4. ClusterIP service routes to port 3100 (BLS)
5. Headless service routes to port 7100 (relay WebSocket)
6. NetworkPolicy allows: agent-runtime-core → agent-society:3100, peer relay → agent-society:7100
7. NetworkPolicy allows: agent-society → connector:8081, agent-society → agent-runtime-core:3100
8. PodDisruptionBudget set to minAvailable: 1
9. Security context matches existing patterns (non-root, read-only FS)
10. Connector configmap updated with LOCAL_DELIVERY fields
11. K8s deployment order documented: TigerBeetle → agent-society → agent-runtime-core → connector

---

### Story 23.3: Deploy Script `--unified` Flag and Bootstrap Verification

**As a** developer,
**I want** a `--unified` flag on the deploy script that starts the full 3-layer stack and verifies bootstrap,
**so that** I can deploy and validate the unified network with a single command.

**Scope:**

- **Add `--unified` flag to `scripts/deploy-5-peer-multihop.sh`:**
  - Uses `docker-compose-unified.yml` instead of `docker-compose-5-peer-multihop.yml`
  - Builds 3 images: `connector` (connector), `agent-runtime-core` (middleware Dockerfile), `agent-society` (from agent-society repo — configurable path via `AGENT_SOCIETY_PATH` env var, default `../agent-society`)
  - Generates Nostr keypairs if not present in `.env.peers` (using `openssl rand -hex 32` for secret keys, derive pubkeys)
- **Phased startup with verification:**
  - Phase 1: Start TigerBeetle + agent-society containers → wait for BLS `/health` healthy on ports 3110-3114 + relay WebSocket listening on 7110-7114
  - Phase 2: Start agent-runtime middleware → wait for `/health` healthy on ports 3200-3204 (including `btpConnected: false` — BTP connects when connector starts)
  - Phase 3: Start connectors → wait for `/health` healthy on ports 9080-9084 + Admin API ready on 8181-8185
  - Phase 4: Bootstrap verification — poll for relay discovery:
    - Verify agent-society-1 published kind:10032 (check relay health or logs)
    - Verify peers 2-5 registered peer1 via `GET /admin/peers` on each connector (8182-8185 should list peer1)
    - Verify 0-amount SPSP handshakes completed (check agent-society logs for `bootstrap:channel-opened` events or poll `GET /admin/channels` on connectors)
  - Phase 5: Verify payment channels opened — `GET /admin/channels` on each connector should return at least 1 channel
  - Phase 6: Verify routing tables populated — `GET /admin/peers` on peer1 should list peers 2-5
  - Phase 7: Send end-to-end test packet — `POST /ilp/send` on agent-runtime-1 with destination `g.peer5`, amount `1000`, verify FULFILL response
- **Status reporting:**
  - Print table of service health status
  - Print bootstrap phase progress
  - Print channel status per peer
  - Print end-to-end test result
  - Exit code 0 on success, 1 on failure

**Acceptance Criteria:**

1. `./scripts/deploy-5-peer-multihop.sh --unified` starts all 16 services
2. Script builds all 3 required Docker images (connector, middleware, agent-society)
3. `AGENT_SOCIETY_PATH` env var configurable (default `../agent-society`)
4. Nostr keypair generation works if `.env.peers` lacks NOSTR keys
5. Phased startup respects dependency chain (no connector before middleware is healthy)
6. Bootstrap verification detects failed handshakes and reports them
7. Channel verification queries Admin API and reports channel status
8. End-to-end test packet sends via `POST /ilp/send` and verifies FULFILL
9. Script prints clear pass/fail status for each verification phase
10. `--unified` flag coexists with existing flags (`--with-agent`, etc.) without conflict
11. Cleanup on `Ctrl+C` (SIGINT trap) tears down unified compose stack

### Story 23.4: Fix Deploy Script — Remove Stale SPSP Test, Enhance Channel Verification, Verify Unified Compose

**As a** developer,
**I want** the deploy script to have accurate post-Epic-22 tests and robust channel verification,
**so that** the `--with-agent` and `--unified` test suites produce reliable pass/fail results.

**Scope:**

- Remove stale SPSP endpoint test, replace with `POST /ilp/send` test
- Enhance Phase 5 channel verification with JSON parsing (jq) and strict failure on zero channels
- Validate `docker-compose-unified.yml` exists, validates, has 16 services, and resolves env vars

**Acceptance Criteria:**

1. Stale SPSP test removed, replaced with ILP send test
2. Phase 5 uses `jq` for proper JSON validation and channel counting
3. Phase 5 fails (not warns) on zero channels
4. Unified compose file validated at script start
5. All existing deploy flags continue to work

**Priority:** P3 — Low (testing & documentation)

---

## Compatibility Requirements

- [x] **Existing Docker Compose files unchanged** — `docker-compose-5-peer-multihop.yml` and others are not modified
- [x] **Existing deploy script flags preserved** — `--with-agent`, `--with-nostr-spsp`, `--agent-only` continue to work
- [x] **Existing K8s manifests unchanged** — `k8s/agent-runtime/`, `k8s/connector/`, `k8s/tigerbeetle/` not modified (except connector configmap addition)
- [x] **`.env.peers` backward compatible** — new fields are additive; existing EVM keys preserved
- [x] **Port allocations don't conflict** — unified compose uses distinct host port ranges

## Risk Mitigation

**Primary Risk:** 16-service Docker Compose strains local resources (memory, CPU).

**Mitigation:**

- TigerBeetle: 4GB cap (existing), connectors: ~256MB each, agent-runtime: ~128MB each, agent-society: ~128MB each
- Total estimated: ~4GB + 5×256MB + 5×128MB + 5×128MB ≈ 5.9GB — within most dev machine capability
- Script can accept `--peers N` flag (future) to reduce to 3 peers for constrained machines
- Services use `restart: unless-stopped` for resilience

**Secondary Risk:** Bootstrap timing — agent-society may attempt handshakes before connectors are ready.

**Mitigation:**

- Dependency chain ensures connectors start last (after BLS + middleware are healthy)
- Bootstrap service has built-in retry logic with exponential backoff
- Deploy script verifies each phase before proceeding to the next
- 120-second timeout per phase with clear failure reporting

**Tertiary Risk:** Agent-society repo not found at expected path.

**Mitigation:**

- Script checks `AGENT_SOCIETY_PATH` exists before building
- Clear error message if missing: "agent-society repo not found at {path}. Set AGENT_SOCIETY_PATH env var."
- Alternatively, pre-built `agent-society` image can be pulled from registry

**Rollback Plan:**

1. `docker compose -f docker-compose-unified.yml down` — tears down all 16 services
2. Remove `--unified` flag usage — revert to `docker-compose-5-peer-multihop.yml` for connector-only testing
3. K8s: `kubectl delete -k k8s/agent-society/` removes all agent-society resources
4. Revert connector configmap if LOCAL_DELIVERY not desired

## Definition of Done

- [ ] All 4 stories completed with acceptance criteria met
- [ ] `docker compose -f docker-compose-unified.yml up` starts 16 healthy services
- [ ] Bootstrap completes: relay discovery, peer registration, SPSP handshakes, channel opening
- [ ] End-to-end test packet fulfilled through full stack
- [ ] K8s manifests apply cleanly with `kubectl apply -k`
- [ ] Connector configmap updated with LOCAL_DELIVERY config
- [ ] Deploy script `--unified` flag runs full verification suite
- [ ] `.env.peers` extended with Nostr keypairs and contract addresses
- [ ] All existing Docker Compose files and deploy script flags unaffected

## Related Work

- **Epic 20:** Bidirectional Agent-Runtime Middleware (provides `POST /ilp/send` used by bootstrap flow)
- **Epic 21:** Payment Channel Admin APIs (provides channel CRUD endpoints verified by deploy script)
- **Epic 22:** Agent-Runtime Middleware Simplification (prerequisite — simplified middleware runs in unified stack)
- **Agent-Society Epic 7:** SPSP Settlement Negotiation (provides kind:23194/23195 handling + channel opening via Admin API)
- **Agent-Society Epic 8:** Nostr Network Bootstrap (provides 3-phase bootstrap flow used in unified deployment)
- **UNIFIED-DEPLOYMENT-PLAN.md:** This epic implements Phases 3-6 of the unified deployment plan
