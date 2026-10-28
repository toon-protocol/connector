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

- **Reads** take `Authorization: Bearer <token>` and nothing more — or, with no
  `Authorization` header, the signature a write takes ([below](#a-signed-read)).
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

## A controller's key: a scope, an expiry and a spending limit

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

[agent]
key            = "<64 hex ed25519 public key>"
scope          = ["packets"]
spending_limit = { per_write = 1000, per_day = 50000 }   # required with `packets`; both figures

[funder]
key           = "<64 hex ed25519 public key>"
scope         = ["peers", "channels-fund"]
deposit_limit = { per_write = 5000000, per_day = 20000000 }   # required with `peers`, `channels-open` or `channels-fund`
```

A scope is a non-empty list of named writes: `packets`, `peers` (`POST /peers`,
`DELETE /peers/:id`), `routes-peers` (`POST /routes/peers`,
`DELETE /routes/peers/:prefix`), `routes-leased`, `channels-open`, `channels-fund`,
`channels-withdraw` and `channels-land`. A `DELETE` belongs to the name of its
`POST`.

**Revoking a key is an edit, with no restart**
([ADR 0079](../adr/0079-a-controllers-key-is-revoked-by-an-edit-and-may-sign-a-read.md)).
The file's contents are read again on every signed request, so an entry you remove is
an unlisted key — `401` — at that key's next request, and an entry you add or change
binds the same way. Only the contents are live: which file `controller_keys_file`
names, and `write_keys` itself, are still read once at boot.

- Replace the file by writing the new list beside it and renaming it into place. A
  file caught half-written, unreadable, or breaking any rule it is held to at boot
  yields **no** restricted keys: each is refused `401` until the file is repaired, and
  the log names the file. The last list that parsed is not served, because that would
  keep honouring a key you may have just tried to revoke. An operator's key is not
  affected.
- The day's totals are kept by key, not by entry. Removing an entry and adding it back
  does not reset its day.
- Whoever can write the file can grant a key at once. Give it the permissions of the
  config that names it.

### A signed read

A controller is handed a key and never the bearer token: the token is shared by every
reader and cannot be revoked for one of them. So a read with **no** `Authorization`
header may carry the signature a write carries — `@method`, `@path` and the
`content-digest` of an empty body — from any key that may sign a write.

- A scope names writes and says nothing about reads: any listed key reads everything
  the bearer token reads.
- A key removed from the file is refused `401`, and one past its `expires` `403`, on
  reads as on writes.
- A signed read is not spent and is not in `GET /audit-log`. It is good until its own
  `expires`, so sign reads with a short one.
- A request that does present an `Authorization` header is judged by the token alone.

A write from a restricted key outside its scope, or after its `expires`, is answered
**`403`** with a body naming the reason (`scope`, `expiry`, `spending-limit` or `deposit-limit`), before anything is
signed, journaled or sent. It is not in `GET /audit-log`; it is a log line and a count
on `toon_writes_refused_total{reason}`. `401` stays the answer to a missing, bad,
replayed or unlisted signature. `GET /write-keys` lists every write key — its key
id, whether it is restricted, its scope, its expiry and — for a key with a spending or a deposit
limit — the limit and what the key has counted today against each.

### The spending limit

A key whose scope includes `packets` states a `spending_limit`: `per_write`, the most one
packet may carry, and `per_day`, the most the key may send in a UTC day. Both are required
together. A `POST /packets` from that key is answered `403` naming the spending limit when
the packet's amount is above `per_write`, or would take the key's total for the current UTC
day above `per_day`. Nothing is signed or sent and no audit record is kept.

- **A packet counts at its full `Prepare` amount once accepted, whatever happens to it** —
  fulfilled, rejected with any code, or never answered. Nothing is ever given back. A
  controller that retries rejected packets uses up its day having spent nothing, and
  `GET /write-keys` is how that is seen. The limit bounds what the key exposed, not what the
  node lost.
- **The figure is bare.** The amount is added as a number, in the base units of whatever
  channel the packet is paid over, with no asset and no conversion. On a node holding tokens
  of different decimals it adds unlike units.
- The count is journaled under `state_dir` (`controller-day-totals.journal`), by key, and
  appended before the packet is handed to the connector; a packet whose count cannot be
  appended is refused. A restart does not reset the day, and neither does removing an entry
  and adding it back. The total starts again at the next UTC day.
- A node that states a `spending_limit` or a `deposit_limit` needs a `state_dir`, and refuses to
  start without one. It also refuses to start when the journal exists but cannot be replayed.
- A key on `write_keys` is never counted or limited. Only packets a write key originates
  count — not packets arriving at the client edge or from a peering.
- The refused-write counter `toon_writes_refused_total` gains the reason `spending-limit`.

The node refuses to start, naming the entry, when a scope is empty or names an
unknown write; a key is not 64 hex characters or appears twice, or is also on
`write_keys` or in `write_keys_file`; a scope lacks the limit it requires, or a limit
states only one of its `per_write` and `per_day`; or the file is missing or
unreadable. An entry whose `expires` has already passed is not a refusal to start.

### The deposit limit

A key whose scope includes `peers`, `channels-open` or `channels-fund` states a
`deposit_limit`: `per_write`, the most one deposit may put into a channel, and `per_day`, the
most the key may deposit in a UTC day. Both are required together. A deposit is answered
`403` naming the deposit limit when it is above `per_write`, or would take the key's deposit
total for the current UTC day above `per_day`. Nothing is journaled as a channel, signed or sent.

- **What counts.** `POST /peers` and `POST /channels` count their `deposit`, when the write opens
  a channel. A repeat that finds an existing channel, or resumes an open already journaled,
  counts nothing. `POST /channels/:id/fund` counts its `amount` every time, because it is an
  increment.
- **When.** A deposit counts when its channel write is journaled, before the transaction is
  sent. A deposit whose transaction then fails is not given back, and a withdrawal gives nothing
  back.
- **A total of its own.** Deposits and packets are counted apart: a key that deposits 100 and
  sends 100 in packets has 100 against each limit, because a deposit stays the node's own money
  and a packet does not.
- **The figure is bare**, as the spending limit's is: the amount is added in the base units of
  the channel it is paid over, with no asset and no conversion.
- The count goes in the same journal under `state_dir` as the spending limit's, with the same
  rules: appended before the write proceeds, refused if it cannot be appended, kept by key,
  surviving a restart and starting again at the next UTC day. A key on `write_keys` is never
  counted.
- `toon_writes_refused_total` gains the reason `deposit-limit`.

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
| `GET /write-keys`    | Every write key: restricted or not, scope, expiry, limit. |
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

| Endpoint                      | Does                                                                                                       |
| ----------------------------- | ---------------------------------------------------------------------------------------------------------- |
| `POST /packets`               | Originate a packet from this node.                                                                         |
| `POST /peers`                 | Establish a peering from a URL. `DELETE /peers/:id` removes it.                                            |
| `POST /routes/peers`          | Write a durable runtime route. `DELETE /routes/peers/:prefix` removes it.                                  |
| `POST /routes/leased`         | Push a TTL-bound route that lapses on its own.                                                             |
| `POST /channels`              | Open an **outbound** x402 channel toward a counterparty, on its published terms.                           |
| `POST /channels/:id/fund`     | **Top up** an outbound channel by an increment.                                                            |
| `POST /channels/:id/withdraw` | Start a withdrawal (EVM) or close (Solana); call again once due to finish.                                 |
| `POST /channels/:id/land`     | Land the latest voucher held on an **inbound** channel now; the answer includes the landing `transaction`. |

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
