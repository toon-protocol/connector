# The operator surface

How you inspect a running node and how you move its money.

It mounts **only** when `[operator]` is configured, and merges onto
`client_edge_addr` — there is no second port and no second listener.

```toml
[operator]
bearer_token_file = "/app/data/operator-bearer-token"
write_keys_file   = "/app/data/operator-write-keys"
```

Each setting is spelled as **exactly one of** a literal or a path:
`bearer_token` / `bearer_token_file`, `write_keys` / `write_keys_file`. The file
forms are the deployed forms, because a fleet's config files are committed to a
public repository and a literal cannot be.

## Read and write are different authorities

The rules are numbered in
[`operator-spec.md`](../protocol/operator-spec.md) and follow from
[ADR 0008](../adr/0008-operator-surface-splits-read-from-write.md):

- **Reads** take `Authorization: Bearer <token>` and nothing more.
- **Writes** take an RFC 9421 HTTP Message Signature from an ed25519 key on
  `write_keys`, with the body bound by an RFC 9530 `Content-Digest`.
- **A bearer token is never sufficient to move value** (OP-03). Read authority
  must not confer write authority.
- **Every write is attributable and individually revocable** (OP-02). A shared
  secret is neither: it cannot say which operator did a thing, and losing it
  loses everything at once.
- **An accepted write cannot be replayed** (OP-05). Signatures carry `created`
  and `expires`, and an accepted signature is remembered until its own expiry.
- **A surface with neither half authenticated refuses to start** (OP-04).

`write_keys` holds only **public** halves. The private half lives with whoever is
calling and never on the node. `connector send --operator-key <file>
--print-keyid` prints the value that goes in the allowlist, derived by the binary
that will do the signing.

## A controller's key: a scope and an expiry

`write_keys` holds an operator's keys, and each may make **every** write. Automation
you point at a node — a controller — should hold less. Declare its key in a file of
its own ([ADR 0078](../adr/0078-a-controllers-write-key-carries-a-scope-and-two-limits.md)):

```toml
[operator]
bearer_token_file    = "/app/data/operator-bearer-token"
write_keys_file      = "/app/data/operator-write-keys"
controller_keys_file = "/app/data/operator-controller-keys"
```

```toml
# /app/data/operator-controller-keys — one table per restricted key
[lander]
key     = "<64 hex ed25519 public key>"
scope   = ["channels-land", "routes-peers"]
expires = "2026-12-31T00:00:00Z"   # optional, RFC 3339
```

A scope is a non-empty list of named writes: `packets`, `peers` (`POST /peers`,
`DELETE /peers/:id`), `routes-peers` (`POST /routes/peers`,
`DELETE /routes/peers/:prefix`), `routes-leased`, `channels-open`, `channels-fund`,
`channels-withdraw` and `channels-land`. A `DELETE` belongs to the name of its
`POST`. The file is read once at boot: changing or revoking a key is an edit and a
restart.

A write from a restricted key outside its scope, or after its `expires`, is answered
**`403`** with a body naming the reason (`scope` or `expiry`), before anything is
signed, journaled or sent. It is not in `GET /audit-log`; it is a log line and a count
on `toon_writes_refused_total{reason}`. `401` stays the answer to a missing, bad,
replayed or unlisted signature. `GET /write-keys` lists every write key — its key
id, whether it is restricted, its scope and its expiry.

The node refuses to start, naming the entry, when a scope is empty or names an
unknown write; a key is not 64 hex characters or appears twice, or is also on
`write_keys` or in `write_keys_file`; a scope lacks the limit it requires, or a limit
states only one of its `per_write` and `per_day`; or the file is missing or
unreadable. An entry whose `expires` has already passed is not a refusal to start.

**Limits are not enforced yet.** An entry that states a `spending_limit` or a
`deposit_limit` is refused at boot, so no release accepts a limit it does not hold.
Scopes that require one — `packets` (a spending limit), `peers`, `channels-open` and
`channels-fund` (a deposit limit) — therefore cannot be declared until they are.

## Reads

| Endpoint             | Answers                                                   |
| -------------------- | --------------------------------------------------------- |
| `GET /peers`         | The peerings this node holds, config and runtime alike.   |
| `GET /routes`        | The full routing table, with each row's source.           |
| `GET /routes/leased` | TTL-bound pushed routes that lapse on their own.          |
| `GET /routes/peers`  | The durable runtime peer-route table.                     |
| `GET /channels`      | Every channel this node knows, with deposits and status.  |
| `GET /claims`        | The claim journal — what you have been paid, and by whom. |
| `GET /identity`      | This node's operator-facing identity.                     |
| `GET /audit-log`     | Every accepted write, with the key that made it.          |
| `GET /write-keys`    | Every write key: restricted or not, its scope and expiry. |
| `GET /metrics`       | Prometheus text.                                          |
| `GET /packets`       | Recent packets, newest first; in memory (ADR 0077).       |

`/metrics` is a bearer-gated read like any other, and there is no unauthenticated
metrics path: absent `[operator]`, `/metrics` is not mounted and answers 404
rather than 401. A _public_ status page — one strangers load — therefore needs a
server-side holder for the token, never the token embedded in the page.

`GET /packets` keeps nothing unless `[operator] packet_history = n` asks it to
hold the last `n` packets. It is a window for watching, lost on restart, and not
a record of anything.

There is also no dedicated health route. What a container healthcheck probes
instead is `GET /ilp/identity` on the client edge, which is free and
unauthenticated and answers only once the config loaded, every settlement backend
connected and the router is serving.

The counters are `toon_packets_total`, `toon_packets_rejected_total`,
`toon_fees_earned_total`, `toon_settlement_total`, and `toon_exposure`, which is
always zero and kept only so scrape configs do not break.

## The dashboard

`GET /dashboard` is the operator's own view of all of the above on one page the
node serves (ADR 0066): packet traffic and rejects by code, fees earned, inbound
and outbound claims, peerings and channels, every route with its source, and
the audit log. It needs no token to load, because it holds nothing. Paste the
bearer token in and it reads; paste an operator key in and it can peer, write a
runtime route or lease one, signing each write in your browser exactly as
`connector send` would. The key stays in the tab's memory — never stored, never
sent — and config-file rows are shown with no button, because a price or a fee
still changes by editing the file and restarting. Reach the page the way you
reach `/metrics` on that box: on the fleet, an SSH tunnel to `client_edge_addr`.

## Writes

| Endpoint                      | Does                                                                             |
| ----------------------------- | -------------------------------------------------------------------------------- |
| `POST /packets`               | Originate a packet from this node.                                               |
| `POST /peers`                 | Establish a peering from a URL. `DELETE /peers/:id` removes it.                  |
| `POST /routes/peers`          | Write a durable runtime route. `DELETE /routes/peers/:prefix` removes it.        |
| `POST /routes/leased`         | Push a TTL-bound route that lapses on its own.                                   |
| `POST /channels`              | Open an **outbound** x402 channel toward a counterparty, on its published terms. |
| `POST /channels/:id/fund`     | **Top up** an outbound channel by an increment.                                  |
| `POST /channels/:id/withdraw` | Start a withdrawal (EVM) or close (Solana); call again once due to finish.       |
| `POST /channels/:id/land`     | Land the latest voucher held on an **inbound** channel now.                      |

The watchers land your inbound vouchers on their own; `land` is the lever for
planned maintenance. `POST /channels` journals the channel before it spends, so a
retry after an error resumes that channel rather than opening a second
(ADR 0075). The `toon-channel` writes `redeem`, `redeem-latest`, `settle`,
`close` and `cooperative-close` are gone; a node draining channels left over
from before ADR 0075 does so on the last release that had them — see
[`docs/operators/draining-toon-channels.md`](../operators/draining-toon-channels.md).

Channel operations answer **503** when no x402 batch-settlement backend is configured.

`POST`/`DELETE` on `/peers*` and `/routes/peers*` are the durable runtime table.
Unlike a leased route, they survive a restart — and they are **refused outright**,
never silently accepted as a shadow, when they would collide with a row the
config file already owns.

## Signing a write

The signature covers exactly three components — `@method`, `@path` and
`content-digest` — with `alg="ed25519"` and `keyid` set to the signer's own
ed25519 public key in hex.

[`docs/operators/sign-write.sh`](../operators/sign-write.sh) is the shipped
signer: `bash` and `openssl`, no other dependency. `-u <base-url>` makes it send
the signed request and print the response; omit `-u` and it prints only the three
headers, for a caller assembling its own request.

```bash
docs/operators/sign-write.sh -k operator-write.key -X POST -p /peers \
    -b "$BODY" -u https://your-node.example
```

[`docs/operators/signing-a-write.md`](../operators/signing-a-write.md) is the
worked example and explains what each computed value is.

`connector send` signs one write of its own — `POST /packets` — and
`--expect-fulfill` makes a non-fulfilled packet a non-zero exit, which is what
turns a rehearsal into a gate.
