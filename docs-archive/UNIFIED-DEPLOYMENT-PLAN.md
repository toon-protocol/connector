# Agent-Runtime + Agent-Society Unified Deployment Plan

## Context

**Goal:** Use agent-society as the first BLS implementation to facilitate the first complete agent-runtime production deployment.

**Problem:** The two projects exist as separate codebases with separate Docker Compose files. There's no unified deployment, and K8s manifests for agent-society are entirely missing.

**Key design decisions:**

- **Three-layer architecture, no STREAM.** The agent-runtime middleware sits between connector and BLS but is stripped of STREAM sessions and SPSP. It just forwards payments to the BLS and computes fulfillment.
- **Fulfillment = SHA256(toon_bytes).** Computed by agent-runtime (not BLS). Ties fulfillment to the exact transmitted payload.
- **BLS is simple.** Receives a payment, validates the event data, stores it, returns accept/reject with optional response data. No fulfillment, no protocol complexity, no blockchain SDKs.
- **Bidirectional middleware.** Agent-runtime proxies both inbound (connector → BLS) and outbound (BLS → connector) ILP packets. The BLS can send packets via `POST /ilp/send`.
- **BLS negotiates, connector executes.** The BLS handles settlement negotiation policy (which chain, which token, accept/reject). The connector owns all payment channel infrastructure (opening, closing, funding, claim signing, BTP claim exchange). The BLS drives channel operations through the connector's Admin API — no blockchain SDK imports in BLS code.
- **Settlement negotiation in SPSP.** The SPSP handshake (kind:23194/23195) negotiates settlement chain. The BLS opens payment channels via connector Admin API (`POST /admin/channels`). ILP FULFILL = channel is live.
- **Bootstrap uses 0-amount ILP packets.** Initial SPSP to the bootstrap node uses the ILP path with amount=0, not direct relay writes. All subsequent communication is paid.

```
                    ILP Network (BTP)
                         ↕
              Connector (routing, settlement, payment channels)
                         ↕ POST /ilp/packets (inbound)
                         ↕ BTP inject (outbound)
              Agent-Runtime middleware (bidirectional proxy)
                 ↓ POST /handle-payment (inbound to BLS)
                 ↑ POST /ilp/send (outbound from BLS)
              Agent-Society BLS
                + Bootstrap Service (Nostr relay reads → peer registration → ILP SPSP)
                + Settlement Negotiator (SPSP chain negotiation → channel open via connector Admin API)
                + Nostr Relay (WebSocket, port 7100)
```

---

## Phase 1: Simplify Agent-Runtime Middleware (remove STREAM)

**Package:** `agent-runtime/packages/agent-runtime/`

The existing agent-runtime middleware has STREAM session management and SPSP that we don't need. Strip it down to a thin forwarder.

### Changes to `src/packet/packet-handler.ts`:

- **Remove** session lookup (`sessionManager.getSessionByAddress`)
- **Remove** STREAM condition verification (`verifyCondition`)
- **Remove** STREAM fulfillment computation (`computeFulfillment(session.sharedSecret, ...)`)
- **Add** direct fulfillment from packet data: `fulfillment = SHA256(Buffer.from(request.data, 'base64'))`
- **Keep** calling `businessClient.handlePayment(paymentRequest)` — this is the BLS call
- **Keep** the reject/fulfill response wrapping into `LocalDeliveryResponse`

New simplified flow:

```
1. Receive LocalDeliveryRequest from connector
2. Build PaymentRequest { paymentId: generated, destination, amount, expiresAt, data }
3. Call BLS: POST /handle-payment → { accept, data?, rejectReason? }
4. If accepted: fulfillment = SHA256(base64decode(request.data))
5. Return { fulfill: { fulfillment, data: response.data } }
6. If rejected: Return { reject: { code, message, data: response.data } }
```

The BLS can return a `data` field (base64) in both accept and reject responses. The middleware passes it through into the ILP fulfill/reject packet, which carries it back to the sender through the network. This enables the BLS to send application-level response data (e.g., SPSP response events, receipts, error details) back to the originating peer.

### Changes to `src/session/session-manager.ts`:

- **Remove** or leave unused. No sessions needed.

### Changes to `src/spsp/spsp-server.ts`:

- **Remove** SPSP HTTP endpoints. Agent-society handles SPSP via Nostr (kind:23194/23195).

### Changes to `src/stream/fulfillment.ts`:

- **Replace** STREAM-based `computeFulfillment(sharedSecret, data)` with `computeFulfillment(toonBytes)` = SHA256(toonBytes)
- **Remove** `verifyCondition(sharedSecret, data, condition)`

### Changes to `src/http/http-server.ts`:

- **Remove** SPSP route registrations (`/.well-known/pay`, `/pay`)
- **Add** `POST /ilp/send` — outbound send endpoint for BLS (see Epic 20, Story 20.1)
- **Keep** `POST /ilp/packets` and `GET /health`

### Changes to `src/agent-runtime.ts`:

- **Remove** SessionManager initialization
- **Remove** SPSP server initialization
- **Add** OutboundBTPClient initialization (connects to connector BTP endpoint for outbound packet injection)
- **Keep** PacketHandler + BusinessClient + HTTP server

### Changes to `src/types/index.ts`:

- **Remove** `PaymentSession`, `SPSPResponse`, `PaymentSetupRequest`, `PaymentSetupResponse`
- **Add** `SendRequest` (`destination`, `amount`, `data`, `timeoutMs`) and `SendResponse` (`fulfilled`, `fulfillment?`, `code?`, `message?`, `data?`)
- **Keep** `PaymentRequest`, `PaymentResponse`, `LocalDeliveryRequest`, `LocalDeliveryResponse`, `AgentRuntimeConfig`
- **Remove** `spspEnabled`, `sessionTtlMs` from `AgentRuntimeConfig`
- **Add** `connectorBtpUrl` to `AgentRuntimeConfig`

---

## Phase 2: Simplify Agent-Society BLS (remove fulfillment)

**File:** `agent-society/docker/src/entrypoint.ts`

The BLS `/handle-payment` endpoint currently computes fulfillment and returns it. Remove that — the BLS just validates and returns accept/reject.

### Changes to `/handle-payment` endpoint:

**Remove** from accept responses:

```typescript
// REMOVE: fulfillment: generateFulfillment(event.id),
```

**Update** accept response to match agent-runtime's `PaymentResponse`:

```typescript
// Accept (simple):
{ accept: true }

// Accept with response data (e.g., SPSP response, receipt):
{ accept: true, data: "base64-encoded-response-data" }
```

The `data` field is passed through the middleware into the ILP Fulfill packet and delivered back to the sender. Use this for SPSP response events (kind:23195), receipts, or any application-level response.

**Update** reject responses to nest under `rejectReason` (matching agent-runtime's expected format):

```typescript
// Current (flat):
{ accept: false, code: "F06", message: "Insufficient payment" }

// New (nested, with optional data):
{ accept: false, rejectReason: { code: "insufficient_funds", message: "Insufficient payment" }, data: "base64-error-details" }
```

The `data` field on rejects is also passed through into the ILP Reject packet.

Use agent-runtime's business-level codes from `REJECT_CODE_MAP` (`packages/agent-runtime/src/types/index.ts`):

- `insufficient_funds` (maps to T04) — for insufficient amount
- `invalid_request` (maps to F00) — for bad TOON data, missing fields
- `internal_error` (maps to T00) — for unexpected errors

**Remove** `generateFulfillment()` function entirely.

**Keep** all validation logic: TOON decode, price calculation, event storage, SPSP request handling (kind:23194).

---

## Phase 3: Unified Docker Compose

**New file:** `agent-runtime/docker-compose-unified.yml`

### Services (16 total):

| Service                             | Image                                   | Ports (host)                                     | Role                    |
| ----------------------------------- | --------------------------------------- | ------------------------------------------------ | ----------------------- |
| `tigerbeetle`                       | ghcr.io/tigerbeetle/tigerbeetle:0.16.68 | 3000                                             | Accounting DB           |
| `peer1`-`peer5`                     | agent-runtime (connector)               | BTP 3000-3004, Health 9080-9084, Admin 8181-8185 | ILP routing             |
| `agent-runtime-1`-`agent-runtime-5` | agent-runtime-core                      | 3200-3204                                        | Thin middleware         |
| `agent-society-1`-`agent-society-5` | agent-society                           | BLS 3110-3114, Relay 7110-7114                   | BLS + relay + bootstrap |

### Startup dependency chain:

```
tigerbeetle (service_started)
  → agent-society-{N} (service_healthy via GET /health on :3100)
    → agent-runtime-{N} (service_healthy via GET /health on :3100)
      → peer{N} (service_healthy via GET /health on :8080)
```

### Key environment wiring per peer (example peer1):

**peer1 (connector):**

```yaml
LOCAL_DELIVERY_ENABLED: 'true'
LOCAL_DELIVERY_URL: http://agent-runtime-1:3100
ADMIN_API_ENABLED: 'true'
# Admin API exposes: /admin/peers, /admin/routes, /admin/channels, /admin/balances, /admin/settlement
# BLS uses CONNECTOR_ADMIN_URL to reach this on port 8181
```

**agent-runtime-1:**

```yaml
PORT: '3100'
BASE_ADDRESS: g.peer1
BUSINESS_LOGIC_URL: http://agent-society-1:3100
CONNECTOR_BTP_URL: ws://peer1:3000
```

**agent-society-1 (bootstrap node):**

```yaml
NODE_ID: agent-society-1
NOSTR_SECRET_KEY: ${PEER1_NOSTR_SECRET_KEY}
ILP_ADDRESS: g.peer1
BTP_ENDPOINT: ws://peer1:3000
CONNECTOR_ADMIN_URL: http://peer1:8181
AGENT_RUNTIME_URL: http://agent-runtime-1:3100
ASSET_CODE: USD
ASSET_SCALE: '6'
BASE_PRICE_PER_BYTE: '10'
# Bootstrap node accepts 0-amount SPSP
SPSP_MIN_PRICE: '0'
# Settlement capabilities
SUPPORTED_CHAINS: 'evm:base:8453'
SETTLEMENT_ADDRESS_EVM_BASE: ${PEER1_EVM_ADDRESS}
PREFERRED_TOKEN_EVM_BASE: ${AGENT_TOKEN_ADDRESS}
TOKEN_NETWORK_EVM_BASE: ${TOKEN_NETWORK_ADDRESS}
SETTLEMENT_TIMEOUT: '86400'
INITIAL_DEPOSIT: '1000000'
# Bootstrap node has no known peers (it IS the bootstrap)
KNOWN_PEERS: '[]'
```

**agent-society-2 (non-bootstrap peer):**

```yaml
NODE_ID: agent-society-2
NOSTR_SECRET_KEY: ${PEER2_NOSTR_SECRET_KEY}
ILP_ADDRESS: g.peer2
BTP_ENDPOINT: ws://peer2:3001
CONNECTOR_ADMIN_URL: http://peer2:8182
AGENT_RUNTIME_URL: http://agent-runtime-2:3100
ASSET_CODE: USD
ASSET_SCALE: '6'
BASE_PRICE_PER_BYTE: '10'
SUPPORTED_CHAINS: 'evm:base:8453'
SETTLEMENT_ADDRESS_EVM_BASE: ${PEER2_EVM_ADDRESS}
PREFERRED_TOKEN_EVM_BASE: ${AGENT_TOKEN_ADDRESS}
TOKEN_NETWORK_EVM_BASE: ${TOKEN_NETWORK_ADDRESS}
SETTLEMENT_TIMEOUT: '86400'
INITIAL_DEPOSIT: '1000000'
KNOWN_PEERS: |
  [{"pubkey": "${PEER1_NOSTR_PUBKEY}", "relayUrl": "ws://agent-society-1:7100", "btpEndpoint": "ws://peer1:3000"}]
```

### Network:

Single `agent-network` bridge. All 16 services share it.

### kind:10032 Event Schema (ILP Peer Info):

Each peer publishes a kind:10032 event to its Nostr relay advertising its ILP and settlement capabilities:

```json
{
  "kind": 10032,
  "content": {
    "ilpAddress": "g.peer1",
    "btpEndpoint": "ws://peer1:3000",
    "assetCode": "USD",
    "assetScale": 6,
    "supportedChains": ["evm:base:8453"],
    "settlementAddresses": {
      "evm:base:8453": "0xPEER1_EVM_ADDRESS"
    },
    "preferredTokens": {
      "evm:base:8453": "0xAGENT_TOKEN_ADDRESS"
    },
    "tokenNetworks": {
      "evm:base:8453": "0xTOKEN_NETWORK_ADDRESS"
    }
  }
}
```

### SPSP Settlement Negotiation (kind:23194/23195):

All SPSP handshakes negotiate settlement and open payment channels. The ILP FULFILL is synchronous proof that the channel is live.

**Request (kind:23194) — TOON-encoded as ILP PREPARE data:**

```json
{
  "kind": 23194,
  "content": {
    "requestId": "uuid",
    "timestamp": 1700000000,
    "ilpAddress": "g.peer2",
    "supportedChains": ["evm:base:8453"],
    "settlementAddresses": { "evm:base:8453": "0xPEER2..." },
    "preferredTokens": { "evm:base:8453": "0xAGENT..." }
  }
}
```

**Response (kind:23195) — returned in ILP FULFILL data:**

```json
{
  "kind": 23195,
  "content": {
    "requestId": "uuid",
    "destinationAccount": "g.peer1",
    "sharedSecret": "base64...",
    "negotiatedChain": "evm:base:8453",
    "settlementAddress": "0xPEER1...",
    "tokenAddress": "0xAGENT...",
    "tokenNetworkAddress": "0xTOKEN_NETWORK...",
    "channelId": "0xCHANNEL_ID...",
    "settlementTimeout": 86400
  }
}
```

ILP packet `expiresAt` must account for on-chain confirmation time (Base L2 ~2-4s, Ethereum L1 ~minutes).

### Connector Admin API — BLS Control Surface:

The BLS communicates with the connector through two API surfaces:

**Via agent-runtime (Epic 20):**

- `POST /ilp/send` on agent-runtime — send outbound ILP packets (SPSP handshakes, peer announcements)

**Directly to connector (Epic 20 + Epic 21):**

- `POST /admin/peers` — register a peer with BTP endpoint and settlement config
- `GET /admin/peers` — list registered peers with connection status
- `DELETE /admin/peers/:peerId` — remove a peer

- `POST /admin/channels` — open a payment channel
  ```json
  // Request:
  { "peerId": "peer1", "chain": "evm:base:8453", "token": "0xAGENT...",
    "tokenNetwork": "0xTOKEN_NETWORK...", "initialDeposit": "1000000", "settlementTimeout": 86400 }
  // Response:
  { "channelId": "0xCHANNEL_ID...", "chain": "evm:base:8453", "status": "open", "deposit": "1000000" }
  ```
- `GET /admin/channels` — list all channels (optional filters: `?peerId=`, `?chain=`, `?status=`)
- `GET /admin/channels/:channelId` — get channel state (on-chain query for freshness)
- `POST /admin/channels/:channelId/deposit` — add funds to a channel
  ```json
  // Request:
  { "amount": "500000" }
  // Response:
  { "channelId": "0x...", "newDeposit": "1500000", "status": "open" }
  ```
- `POST /admin/channels/:channelId/close` — close a channel (cooperative by default)
  ```json
  // Request:
  { "cooperative": true }
  // Response:
  { "channelId": "0x...", "status": "closing", "txHash": "0x..." }
  ```
- `GET /admin/balances/:peerId` — query TigerBeetle balance for a peer
  ```json
  // Response:
  {
    "peerId": "peer1",
    "balances": [
      { "tokenId": "USD", "debitBalance": "5000", "creditBalance": "3000", "netBalance": "2000" }
    ]
  }
  ```
- `GET /admin/settlement/states` — query settlement monitor health

**SPSP flow using Admin APIs:**

```
BLS receives kind:23194 via /handle-payment:
  1. Parse SPSP request, extract supportedChains + settlementAddresses
  2. Compute chain intersection with own supportedChains
  3. POST /admin/channels on CONNECTOR_ADMIN_URL → { channelId, status: "open" }
  4. GET /admin/channels/:channelId → verify state is "open"
  5. Return accept + kind:23195 response data with channelId
     → agent-runtime computes fulfillment, returns ILP FULFILL to sender
```

**Bootstrap flow using Admin APIs:**

```
BLS bootstrap service on startup:
  1. Read kind:10032 from genesis relay (free WebSocket read)
  2. POST /admin/peers on CONNECTOR_ADMIN_URL → register peer with BTP endpoint + settlement
  3. POST /ilp/send on AGENT_RUNTIME_URL → 0-amount SPSP handshake (kind:23194)
  4. Receive FULFILL → extract channelId from kind:23195 response
  5. POST /admin/peers on CONNECTOR_ADMIN_URL → update peer registration with channelId
```

All Admin API calls use `CONNECTOR_ADMIN_URL` (e.g., `http://peer1:8181`). Authentication via optional API key.

### Bootstrap flow after all services healthy:

**Phase 1 — Relay discovery (FREE, passive):**

1. agent-society-1 publishes kind:10032 (ILP peer info + settlement capabilities) to its own Nostr relay
2. agent-society-{2..5} read kind:10032 from agent-society-1's relay (passive WebSocket subscription — free)

**Phase 2 — Connector registration (FREE, local):** 3. agent-society-{2..5} extract peer1's BTP endpoint and settlement info from kind:10032 4. agent-society-{2..5} call `POST /admin/peers` on their own connector to register peer1 (local HTTP call, includes settlement config) 5. peer{2..5} connectors now have a BTP route to peer1

**Phase 3 — Bootstrap SPSP via 0-amount ILP (FREE, but uses ILP path):** 6. agent-society-{2..5} call `POST /ilp/send` on their agent-runtime with:

- destination: `g.peer1`, amount: `"0"`, data: TOON-encoded kind:23194 with settlement preferences

7. Packet routes: peer{N} connector → BTP → peer1 connector → agent-runtime-1 → agent-society-1 BLS
8. Peer1's BLS accepts 0-amount (configured via `SPSP_MIN_PRICE=0`)
9. Peer1's BLS negotiates chain (intersection of supportedChains), opens payment channel via connector Admin API (`POST /admin/channels`) synchronously
10. **ILP FULFILL** returned only after connector confirms channel is open — FULFILL data contains kind:23195 with channelId
11. **ILP REJECT** if chain negotiation fails or channel opening fails
12. agent-society-{2..5} update peer1 registration with channelId from FULFILL response

**Phase 4 — Reverse registration (PAID, ILP-routed through peer1):** 13. agent-society-{2..5} publish their kind:10032 as TOON-encoded ILP PREPARE (amount > 0) routed through peer1 - Peer1 earns routing fees on every announcement 14. agent-society-1 reads new kind:10032 events from its relay 15. agent-society-1 sends paid SPSP (kind:23194) to each new peer via `POST /ilp/send` 16. Each peer's BLS negotiates chain, opens channel via connector Admin API → FULFILL confirms channel 17. agent-society-1 registers each peer via `POST /admin/peers` with settlement config

**Phase 5 — Cross-peer discovery (PAID, ILP-routed):** 18. Peers read relay for other peers' kind:10032 events (free relay reads) 19. All SPSP handshakes between non-bootstrap peers route through ILP (paid, multi-hop) 20. Each handshake = settlement negotiation + channel opening via connector Admin API 21. Network grows organically — each peer-pair has its own negotiated settlement relationship

---

## Phase 4: K8s Manifests for Agent-Society

```
k8s/agent-society/
├── base/
│   ├── namespace.yaml
│   ├── serviceaccount.yaml
│   ├── configmap.yaml          # BLS_PORT, WS_PORT, ASSET_CODE, CONNECTOR_ADMIN_URL
│   ├── secret.yaml             # NOSTR_SECRET_KEY
│   ├── deployment.yaml         # BLS + relay co-located, ports 3100 + 7100
│   ├── service.yaml            # ClusterIP :3100 (BLS) + headless :7100 (relay WS)
│   ├── networkpolicy.yaml
│   ├── pdb.yaml
│   └── kustomization.yaml
├── overlays/
│   ├── staging/
│   │   └── kustomization.yaml
│   └── production/
│       └── kustomization.yaml
└── kustomization.yaml
```

Also create K8s manifests for agent-runtime middleware:

```
k8s/agent-runtime-core/
├── base/
│   ├── deployment.yaml
│   ├── configmap.yaml          # BASE_ADDRESS, BUSINESS_LOGIC_URL
│   ├── service.yaml            # ClusterIP :3100
│   └── kustomization.yaml
└── kustomization.yaml
```

### Key design decisions:

- **Deployment** (not StatefulSet): in-memory SQLite, no persistent state
- **Two Services for agent-society**: ClusterIP for BLS HTTP (connector talks to this), headless for relay WebSocket (peer-to-peer discovery)
- **Cross-namespace networking**: connector namespace needs egress to agent-runtime-core:3100; agent-runtime-core needs egress to agent-society:3100; agent-society needs egress to connector admin:8081
- **Probes**: startup/liveness/readiness on BLS `/health`

### Update existing connector K8s config:

**File:** `k8s/connector/base/configmap.yaml` — add:

```yaml
LOCAL_DELIVERY_ENABLED: 'true'
LOCAL_DELIVERY_URL: 'http://agent-runtime-core.agent-runtime-core.svc.cluster.local:3100'
```

### K8s deployment order:

```bash
kubectl apply -k k8s/tigerbeetle/
kubectl apply -k k8s/agent-society/
kubectl apply -k k8s/agent-runtime-core/
kubectl apply -k k8s/connector/
```

---

## Phase 5: Update Deploy Script

**File:** `agent-runtime/scripts/deploy-5-peer-multihop.sh`

1. **Add `--unified` flag** using `docker-compose-unified.yml`
2. **Image build steps:**
   ```bash
   docker build -t agent-runtime .
   docker build -t agent-runtime-core -f packages/agent-runtime/Dockerfile .
   cd ../agent-society && docker build -f docker/Dockerfile -t agent-society .
   ```
3. **Phased startup:**
   - Phase 1: Start TigerBeetle + agent-society containers, wait for BLS health + relay health
   - Phase 2: Start agent-runtime middleware, wait for health (including BTP client connected)
   - Phase 3: Start connectors, wait for health + Admin API ready (including channel endpoints)
   - Phase 4: Wait for bootstrap — verify relay discovery, peer registration, 0-amount SPSP handshakes, channel opening via Admin API
   - Phase 5: Wait for reverse registration — verify paid kind:10032 announcements, peer1 registers peers 2-5
   - Phase 6: Verify payment channels opened (`GET /admin/channels` on each connector, cross-check with on-chain state)
   - Phase 7: Verify routing tables populated (`GET /admin/peers` on each connector)
   - Phase 8: Verify balances initialized (`GET /admin/balances/:peerId` on each connector)
   - Phase 9: Send end-to-end test packet (g.peer1 → g.peer5, verify FULFILL)

---

## Phase 6: Update .env Configuration

**File:** `agent-runtime/.env.peers` — add Nostr keypairs and settlement addresses:

```env
# Nostr identity (per-peer)
PEER1_NOSTR_SECRET_KEY=<64-char hex>
PEER1_NOSTR_PUBKEY=<derived 64-char hex>
PEER2_NOSTR_SECRET_KEY=<64-char hex>
PEER2_NOSTR_PUBKEY=<derived 64-char hex>
PEER3_NOSTR_SECRET_KEY=<64-char hex>
PEER3_NOSTR_PUBKEY=<derived 64-char hex>
PEER4_NOSTR_SECRET_KEY=<64-char hex>
PEER4_NOSTR_PUBKEY=<derived 64-char hex>
PEER5_NOSTR_SECRET_KEY=<64-char hex>
PEER5_NOSTR_PUBKEY=<derived 64-char hex>

# Settlement addresses (per-peer)
PEER1_EVM_ADDRESS=0x...
PEER2_EVM_ADDRESS=0x...
PEER3_EVM_ADDRESS=0x...
PEER4_EVM_ADDRESS=0x...
PEER5_EVM_ADDRESS=0x...

# Shared contract addresses (same for all peers on same chain)
AGENT_TOKEN_ADDRESS=0x...
TOKEN_NETWORK_ADDRESS=0x...
```

---

## Files to Create/Modify

### New files (agent-runtime repo):

1. `docker-compose-unified.yml`
2. `k8s/agent-society/base/{namespace,serviceaccount,configmap,secret,deployment,service,networkpolicy,pdb,kustomization}.yaml`
3. `k8s/agent-society/{kustomization.yaml,overlays/staging/kustomization.yaml,overlays/production/kustomization.yaml}`
4. `k8s/agent-runtime-core/base/{deployment,configmap,service,kustomization}.yaml`
5. `k8s/agent-runtime-core/kustomization.yaml`

### Modified files (agent-runtime repo):

6. `packages/agent-runtime/src/packet/packet-handler.ts` — remove STREAM, use SHA256(data)
7. `packages/agent-runtime/src/agent-runtime.ts` — remove SessionManager, SPSP; add OutboundBTPClient
8. `packages/agent-runtime/src/http/http-server.ts` — remove SPSP routes; add `POST /ilp/send`
9. `packages/agent-runtime/src/stream/fulfillment.ts` — replace with SHA256(toonBytes)
10. `packages/agent-runtime/src/types/index.ts` — remove STREAM/SPSP types; add SendRequest, SendResponse
11. `packages/connector/src/http/admin-api.ts` — extend AddPeerRequest with settlement fields (Epic 20); add channel CRUD, deposit, close, balance, and settlement state endpoints (Epic 21)
12. `packages/connector/src/settlement/types.ts` — ensure PeerConfig accessible from Admin API
13. `packages/connector/src/settlement/channel-manager.ts` — expose methods for Admin API (ensureChannelExists, getChannelForPeer, getAllChannels, getChannelById)
14. `packages/connector/src/settlement/account-manager.ts` — expose getAccountBalance for Admin API
15. `packages/connector/src/core/connector-node.ts` — wire ChannelManager, AccountManager, SettlementMonitor, ClaimReceiver to Admin API
16. `scripts/deploy-5-peer-multihop.sh` — add `--unified` flag, bootstrap verification steps
17. `.env.peers` — add Nostr keypairs, EVM addresses, contract addresses
18. `k8s/connector/base/configmap.yaml` — add LOCAL_DELIVERY config

### Modified files (agent-society repo):

19. `docker/src/entrypoint.ts` — remove fulfillment from responses, nest reject under `rejectReason`
20. `packages/core/src/types.ts` — extend IlpPeerInfo, SpspRequest, SpspResponse with settlement fields
21. `packages/core/src/events/builders.ts` — updated event builders for settlement schemas
22. `packages/core/src/spsp/NostrSpspServer.ts` — settlement negotiation via connector Admin API (POST /admin/channels)
23. `packages/core/src/bootstrap.ts` — rewrite for ILP-first bootstrap flow using Admin APIs

---

## Verification

### Docker Compose:

```bash
./scripts/deploy-5-peer-multihop.sh --unified
```

Expected: All 16 containers healthy, bootstrap completes all 5 phases, test packet fulfilled end-to-end.

### Component health:

```bash
curl http://localhost:3110/health    # BLS (agent-society-1)
curl http://localhost:3200/health    # Agent-runtime middleware (agent-runtime-1)
curl http://localhost:9080/health    # Connector (peer1)
```

### Outbound send API:

```bash
# Send 0-amount test packet via agent-runtime
curl -X POST http://localhost:3200/ilp/send \
  -H "Content-Type: application/json" \
  -d '{"destination": "g.peer2", "amount": "0", "data": "base64...", "timeoutMs": 5000}'
```

### Payment channel Admin API:

```bash
# List all channels on peer1's connector
curl http://localhost:8181/admin/channels

# Open a channel (what the BLS calls during SPSP negotiation)
curl -X POST http://localhost:8181/admin/channels \
  -H "Content-Type: application/json" \
  -d '{"peerId": "peer2", "chain": "evm:base:8453", "token": "0xAGENT...", "tokenNetwork": "0xTOKEN_NETWORK...", "initialDeposit": "1000000"}'

# Check channel state
curl http://localhost:8181/admin/channels/0xCHANNEL_ID

# Deposit to a channel
curl -X POST http://localhost:8181/admin/channels/0xCHANNEL_ID/deposit \
  -H "Content-Type: application/json" \
  -d '{"amount": "500000"}'

# Query balance with a peer
curl http://localhost:8181/admin/balances/peer2

# Query settlement states
curl http://localhost:8181/admin/settlement/states

# Close a channel
curl -X POST http://localhost:8181/admin/channels/0xCHANNEL_ID/close \
  -H "Content-Type: application/json" \
  -d '{"cooperative": true}'
```

### Bootstrap verification:

```bash
# Check routing tables populated
curl http://localhost:8181/admin/peers   # peer1 should list peers 2-5
curl http://localhost:8182/admin/peers   # peer2 should list peer1 + others

# Check payment channels opened via Admin API
curl http://localhost:8181/admin/channels   # peer1 should have channels to peers 2-5
curl http://localhost:8182/admin/channels   # peer2 should have channel to peer1 + others

# Verify on-chain (alternative to Admin API query)
cast call $TOKEN_NETWORK "getChannelInfo(bytes32)" $CHANNEL_ID --rpc-url $BASE_RPC

# Check balances
curl http://localhost:8181/admin/balances/peer2   # Should show initial state
```

### End-to-end packet:

```bash
npm run send-packet -- -c ws://localhost:3000 -d g.peer5 -a 1000
```

Expected: Packet routes peer1 → peer2 → peer3 → peer4 → peer5, FULFILL returned.

### K8s:

```bash
kubectl apply -k k8s/tigerbeetle/
kubectl apply -k k8s/agent-society/overlays/staging/
kubectl apply -k k8s/agent-runtime-core/
kubectl apply -k k8s/connector/overlays/staging/
kubectl get pods -A
```

### Related Epics:

- **agent-runtime Epic 20:** Bidirectional middleware (`POST /ilp/send`, Admin API settlement fields)
- **agent-runtime Epic 21:** Payment Channel Admin APIs (channel open/close/deposit/query, balance queries)
- **agent-society Epic 7:** SPSP settlement negotiation (kind:10032/23194/23195 schemas, channel opening via connector Admin API)
- **agent-society Epic 8:** Nostr network bootstrap (full bootstrap flow, Docker/deploy integration)
