# Unified Deployment Handoff — Investigating Phase 5 & Phase 9 Failures

## Context

The `--unified` flag on `scripts/deploy-5-peer-multihop.sh` deploys the full 3-layer stack:

- **Layer 1**: 5x agent-society (BLS + Nostr relay) containers
- **Layer 2**: 5x agent-runtime (middleware) containers
- **Layer 3**: 5x connector (peer1-5) containers + TigerBeetle

The deployment runs 9 verification phases. **7/9 pass**, 2 fail.

## Current State (Stack Is Running)

```
docker compose -f docker-compose-unified.yml --env-file .env.peers ps
docker compose -f docker-compose-unified.yml --env-file .env.peers logs -f
```

### Phase Results

| Phase   | Test                     | Result                                                            |
| ------- | ------------------------ | ----------------------------------------------------------------- |
| 1/9     | Agent-Society Health     | PASS — all 5 BLS healthy                                          |
| 2/9     | Agent-Runtime Middleware | PASS — all 5 healthy, BTP connected                               |
| 3/9     | Connector Health         | PASS — all 5 connectors + Admin APIs ready                        |
| 4/9     | Bootstrap Verification   | PASS — kind:10032, relay discovery, peer registration, 3 channels |
| **5/9** | **Reverse Registration** | **FAIL — peer1 doesn't register peers 2-5 back**                  |
| 6/9     | Payment Channels         | PASS — 4 channels open (peer1↔peer2, peer4↔peer3, peer5↔peer4)    |
| 7/9     | Routing Tables           | PASS — full mesh 20/20 routes                                     |
| 8/9     | Balance Initialization   | PASS — 20/20 balance queries responded                            |
| **9/9** | **End-to-End Test**      | **FAIL — "Outbound sender not connected"**                        |

## Failure 1: Phase 5 — Reverse Registration

### What Should Happen

When peers 2-5 bootstrap and send kind:10032 (ILP Peer Info) to peer1's relay, agent-society-1 should:

1. Receive the kind:10032 events from peers 2-5
2. Perform SPSP handshake back to each peer
3. Call `POST /admin/peers` on peer1's connector to register peers 2-5

### What Actually Happens

- Peers 2-5 successfully register peer1 (forward direction works)
- agent-society-1 logs show "reverse registration activity detected"
- But `GET http://localhost:8181/admin/peers` on peer1 shows 0 peers registered by BLS
- peer1 has 4 peers from static config routes, but not from dynamic BLS registration

### Where to Investigate

**agent-society repo** (`../agent-society`):

- `packages/core/src/bootstrap/BootstrapService.ts` — the main bootstrap logic
  - Look at how it handles incoming kind:10032 events
  - Check if `setConnectorAdmin()` is being called before reverse registration
  - Check if `addPeer()` is being called with correct params
- `docker/src/entrypoint.ts` — the entrypoint that wires everything together
  - Check if `bootstrapService.setConnectorAdmin(admin)` is called
  - Check the ConnectorAdminClient implementation (HTTP calls to connector Admin API)
- `packages/core/src/discovery/SocialPeerDiscovery.ts` — social graph peer discovery
  - May be the mechanism for reverse registration
- `packages/core/src/bootstrap/RelayMonitor.ts` — monitors relay for kind:10032

**Useful debug commands:**

```bash
# Check agent-society-1 logs for reverse registration
docker compose -f docker-compose-unified.yml --env-file .env.peers logs agent-society-1 2>&1 | grep -i "reverse\|register\|addPeer\|admin/peers\|POST"

# Check peer1 connector admin API
curl -s http://localhost:8181/admin/peers | jq .

# Check what peers 2-5 have registered
for i in 2 3 4 5; do echo "peer$i:"; curl -s "http://localhost:$((8180+i))/admin/peers" | jq .; done
```

## Failure 2: Phase 9 — End-to-End Packet Send

### What Should Happen

```bash
curl -X POST http://localhost:3200/ilp/send \
  -H 'Content-Type: application/json' \
  -d '{"destination":"g.peer5","amount":"1000","data":"SGVsbG8=","timeoutMs":10000}'
```

Should route: agent-runtime-1 → peer1 → peer2 → ... → peer5 → agent-runtime-5 → FULFILL

### What Actually Happens

```json
{ "error": "Service unavailable", "message": "Outbound sender not connected" }
```

The middleware says the "outbound sender" is not connected. This means the BTP client inside agent-runtime middleware that sends packets TO the connector is not established.

### Contradiction

Phase 2 showed all 5 middlewares as "BTP connected" — so the health check reports BTP connected, but the actual outbound send path isn't working.

### Where to Investigate

**agent-runtime repo** (`packages/agent-runtime/`):

- The middleware's BTP client connection to the connector
- The `/ilp/send` endpoint handler — what checks "outbound sender not connected"?
- The health check vs actual send path — why does health say connected but send says not?

```bash
# Check agent-runtime-1 health in detail
curl -s http://localhost:3200/health | jq .

# Check agent-runtime-1 logs for BTP/connection issues
docker compose -f docker-compose-unified.yml --env-file .env.peers logs agent-runtime-1 2>&1 | grep -i "btp\|connect\|outbound\|sender"

# Try sending a 0-amount test packet
curl -X POST http://localhost:3200/ilp/send \
  -H 'Content-Type: application/json' \
  -d '{"destination":"g.peer1","amount":"0","data":"dGVzdA==","timeoutMs":5000}'
```

**Key files:**

- `packages/agent-runtime/src/` — middleware source
- Look for the error message "Outbound sender not connected" to find the exact code path

## Fixes Already Applied This Session

### agent-runtime repo (this repo)

1. `scripts/deploy-5-peer-multihop.sh` — Fixed agent-society build command to use `-f docker/Dockerfile`
2. `docker-compose-unified.yml` — Added `EXPLORER_PORT` to all 5 peers (peer2 was conflicting 3001 vs btpServerPort)

### agent-society repo (`../agent-society`)

1. `docker/Dockerfile` — Added `python3 make g++` build deps for native modules
2. `packages/core/src/discovery/SocialPeerDiscovery.ts:15` — Fixed import path: `../bootstrap.js` → `../bootstrap/index.js`
3. `packages/core/src/discovery/SocialPeerDiscovery.ts:104` — Changed `subscribeMany` filter from array to single object (nostr-tools 2.23 API change)
4. `packages/core/src/bootstrap/RelayMonitor.ts:124` — Same `subscribeMany` signature fix
5. `packages/core/src/bootstrap/BootstrapService.ts:114` — Added `unknown` intermediate cast for `this` assertion
6. `packages/core/src/bootstrap/agent-runtime-client.ts:61-65` — Changed dot notation to bracket notation for index signature (TS4111)
7. `packages/core/src/spsp/settlement.ts:56` — Changed `intersection[0]` to `intersection[0] ?? null`
8. `docker/tsconfig.json` — Added `"exclude": ["src/**/*.test.ts"]`
9. `docker/src/entrypoint.ts:632` — Added `?? []` for optional `supportedChains`

## Port Map

| Service         | Internal                          | External         |
| --------------- | --------------------------------- | ---------------- |
| agent-society-1 | BLS:3100, WS:7100                 | 3110, 7110       |
| agent-society-2 | BLS:3100, WS:7100                 | 3111, 7111       |
| agent-society-3 | BLS:3100, WS:7100                 | 3112, 7112       |
| agent-society-4 | BLS:3100, WS:7100                 | 3113, 7113       |
| agent-society-5 | BLS:3100, WS:7100                 | 3114, 7114       |
| agent-runtime-1 | 3100                              | 3200             |
| agent-runtime-2 | 3100                              | 3201             |
| agent-runtime-3 | 3100                              | 3202             |
| agent-runtime-4 | 3100                              | 3203             |
| agent-runtime-5 | 3100                              | 3204             |
| peer1           | BTP:3000, Health:8080, Admin:8081 | 3000, 9080, 8181 |
| peer2           | BTP:3001, Health:8080, Admin:8081 | 3001, 9081, 8182 |
| peer3           | BTP:3002, Health:8080, Admin:8081 | 3002, 9082, 8183 |
| peer4           | BTP:3003, Health:8080, Admin:8081 | 3003, 9083, 8184 |
| peer5           | BTP:3004, Health:8080, Admin:8081 | 3004, 9084, 8185 |

## Architecture Reminder

```
Agent-Society (BLS)  →  Agent-Runtime (Middleware)  →  Connector (ILP)
   Nostr relay               BTP client/server           BTP peers
   Bootstrap logic           /ilp/send endpoint          Admin API
   Settlement policy         SHA-256 fulfillment         Payment channels
```

- BLS negotiates settlement policy, connector executes
- BLS drives channel operations through connector Admin API
- Middleware sits between BLS and connector, handling ILP packet send/receive
