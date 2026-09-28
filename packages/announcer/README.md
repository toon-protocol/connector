# @toon-protocol/announcer

A standalone kind:10032 announcer sidecar for the Rust connector's client
edge (connector#681).

## Why this is a separate service

ADR 0022 ("a connector answers, it does not announce") and ADR 0006
("mechanism not policy") forbid the Rust connector from pushing a kind:10032
self-announce itself. This sidecar is the component ADR 0006 anticipated
living "somewhere else": it never links against connector crates, never
touches connector config, and never runs inside the connector process. It
only **asks** the edge's already-public answers —

- `GET /ilp/identity` — the client edge's ADR 0018 identity (informational
  only; this is a different keypair from the one this sidecar signs with).
- The x402 payment-required greeting (`POST /ilp` with no claim header,
  client-edge-spec.md §1.4) — the facts a buyer needs to open a channel. Since
  ADR 0075 (connector#1384) these are read from the greeting's
  `batch-settlement` `accepts[]` entries alone (each chain's CAIP-2
  `network`, `asset` and `payTo`), and the route price from its
  `extensions.toon` terms; there is no `toon-channel` entry and no
  `TokenNetwork` or TOON program to announce, so the announce no longer
  carries `tokenNetworks`. A greeting from a pre-ADR 0075 connector is
  refused (logged) rather than half-read.

— and republishes them as a signed kind:10032 event on a timer (default
300s). No push-announce loop enters the connector binary; ADR 0022 stands.

## What it signs with

The kind:10032 event is signed with this sidecar's **own dedicated announce
identity** — never the Rust edge's ADR 0018 wrap key (that key seals packet
payloads, not Nostr events, and answering `/ilp/identity` is a different
audience entirely per ADR 0022). Wiring the WRONG key here recreates exactly
the dual-announce hazard connector#681 exists to close (a stale identity's
announce competing with the live edge's) — the orchestrator must confirm the
configured key's pubkey (logged at startup) is the CURRENT live identity for
the box being fronted, not a retired one.

## Config / environment surface

| Variable                             | Required | Default                                            | Notes                                                                                                                                                                                                                                                                                                      |
| ------------------------------------ | :------: | -------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ANNOUNCER_IDENTITY_SECRET_KEY_HEX`  |  one of  | —                                                  | 64-char hex secp256k1 secret key, the announce identity. Mutually exclusive with `_FILE`.                                                                                                                                                                                                                  |
| `ANNOUNCER_IDENTITY_SECRET_KEY_FILE` |  one of  | —                                                  | Path to a file containing the same 64-char hex key (mounted read-only, like the edge's own keys).                                                                                                                                                                                                          |
| `ANNOUNCER_RELAY_URLS`               |    no    | _(empty — logs and skips publish)_                 | Comma-separated relay URL(s) to publish to. `ws(s)://` = NIP-01 publish (rejected by the production relay's pay-to-write WS gate); `http(s)://` = the relay's PRIVATE payment-oblivious `POST /write` ingress (the trusted-local free path — use e.g. `http://relay:3100` from inside the deploy network). |
| `ANNOUNCER_RUST_EDGE_URL`            |    no    | `http://connector-rust:4000`                       | **Internal only, never advertised.** Base URL of the Rust client edge to poll.                                                                                                                                                                                                                             |
| `ANNOUNCER_ILP_ADDRESS`              |    no    | `g.toon`                                           | Primary ILP address to advertise.                                                                                                                                                                                                                                                                          |
| `ANNOUNCER_ILP_ADDRESSES`            |    no    | `[ANNOUNCER_ILP_ADDRESS]`                          | Comma list of every address this announce covers, primary first.                                                                                                                                                                                                                                           |
| `ANNOUNCER_ROUTE_PUBLISH`            |    no    | derived: first `*.relay` address, else the primary | The `routes.publish` hint.                                                                                                                                                                                                                                                                                 |
| `ANNOUNCER_ROUTE_STORE`              |    no    | derived: first `*.store` (else `*.ario`) address   | The `routes.store` hint.                                                                                                                                                                                                                                                                                   |
| `ANNOUNCER_PROBE_ROUTES`             |    no    | `ANNOUNCER_ILP_ADDRESSES`                          | Addresses to poll the x402 greeting for (price + settlement terms).                                                                                                                                                                                                                                        |
| `ANNOUNCER_HTTP_ENDPOINT`            |    no    | `https://proxy.devnet.toonprotocol.dev/rust/ilp`   | Public ILP-over-HTTP endpoint advertised (connector#680).                                                                                                                                                                                                                                                  |
| `ANNOUNCER_BTP_ENDPOINT`             |    no    | `wss://proxy.devnet.toonprotocol.dev/rust/ilp/btp` | Public BTP endpoint advertised (connector#680).                                                                                                                                                                                                                                                            |
| `ANNOUNCER_RELAY_PUBLIC_URL`         |    no    | first `ws(s)://` `ANNOUNCER_RELAY_URLS` entry      | The `relayUrl` field advertised for free reads. Never falls back to an `http(s)://` publish entry (that is a private ingress) — set explicitly when publishing over HTTP.                                                                                                                                  |
| `ANNOUNCER_ASSET_CODE`               |    no    | `USDC`                                             | —                                                                                                                                                                                                                                                                                                          |
| `ANNOUNCER_ASSET_SCALE`              |    no    | `6`                                                | —                                                                                                                                                                                                                                                                                                          |
| `ANNOUNCER_SOLANA_CHAIN_ID`          |    no    | `solana:devnet`                                    | The greeting's Solana entry names its chain by CAIP-2 genesis hash; it is announced under this id.                                                                                                                                                                                                         |
| `ANNOUNCER_REFRESH_INTERVAL_SECS`    |    no    | `300`                                              | Republish cadence.                                                                                                                                                                                                                                                                                         |
| `ANNOUNCER_TTL_SECS`                 |    no    | `2 × ANNOUNCER_REFRESH_INTERVAL_SECS`              | NIP-40 expiration TTL stamped on each announce.                                                                                                                                                                                                                                                            |
| `ANNOUNCER_EDGE_POLL_TIMEOUT_MS`     |    no    | `5000`                                             | Per-request timeout polling the edge.                                                                                                                                                                                                                                                                      |
| `ANNOUNCER_PUBLISH_TIMEOUT_MS`       |    no    | `5000`                                             | Per-relay timeout waiting for `OK`.                                                                                                                                                                                                                                                                        |
| `ANNOUNCER_HEALTH_PORT`              |    no    | `8090`                                             | `GET /health` for the docker healthcheck.                                                                                                                                                                                                                                                                  |
| `ANNOUNCER_NOTICE_ID`                |    no    | —                                                  | Operator notice (toon#183). Stable id for the notice — set together with `_SUMMARY`/`_URL` or not at all. Absent means no notice, the common case: no key, no default.                                                                                                                                     |
| `ANNOUNCER_NOTICE_SEVERITY`          |    no    | `info`                                             | `info` or `action-required`. Only meaningful (and only validated) when the notice is configured; an unrecognized value fails startup rather than announcing a bad one.                                                                                                                                     |
| `ANNOUNCER_NOTICE_SUMMARY`           |    no    | —                                                  | One line, plain — the reason to open the notice, not the notice itself.                                                                                                                                                                                                                                    |
| `ANNOUNCER_NOTICE_URL`               |    no    | —                                                  | Where the durable notice text lives. Carried as-is; this sidecar never fetches it.                                                                                                                                                                                                                         |
| `LOG_LEVEL`                          |    no    | `info`                                             | pino level.                                                                                                                                                                                                                                                                                                |

## kind:10032 wire shape

Regular-replaceable (NIP-01, 10000-19999): a relay replaces this node's prior
announce by `(pubkey, kind)` alone. **No `d` tag is ever emitted** — matching
the retired TypeScript connector's own `SelfAnnounceService` exactly. The
only actual Nostr `tags` entry is an optional NIP-40 `["expiration", ...]`;
everything else (endpoints, chains, settlement/contract addresses, prices,
route hints) rides inside the JSON-stringified `content`, same as the
retired service, so any existing kind:10032 consumer needs no changes.

## Running locally

```sh
npm install
ANNOUNCER_IDENTITY_SECRET_KEY_HEX=$(openssl rand -hex 32) \
ANNOUNCER_RELAY_URLS=wss://relay.devnet.toonprotocol.dev \
ANNOUNCER_RUST_EDGE_URL=http://127.0.0.1:4000 \
npm run dev
```
