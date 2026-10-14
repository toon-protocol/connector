# 1. Run a node

The connector is one static binary that reads one TOML file. Three ways to get it running:
`init` writes a devnet node for you, Docker runs the one you write, and source is for
changing the connector rather than running it.

## The fastest start: `connector init --devnet`

One command writes a runnable devnet node into a directory: a config with one priced route,
the Base Sepolia settlement block, an operator surface so you can see what you were paid,
fresh keys owned by you, and a `compose.yml` that runs the connector and a payment-oblivious
echo app as your own uid. It touches no chain and overwrites nothing.

```bash
IMG=ghcr.io/toon-protocol/connector:rust-main   # or a dated release handle that carries `init`
docker run --rm --user "$(id -u):$(id -g)" -v "$PWD:/w" -w /w "$IMG" init --devnet --dir node --image "$IMG"
cd node && docker compose up -d
curl -s http://127.0.0.1:3000/ilp | jq .routes
```

`init` prints what it wrote, the node's settlement address, and the three `toon-client`
commands that pay it. The first-run test of 2026-10-13 took a careful operator twenty minutes
and four traps to reach this point by hand; the traps were a key the image could not read, a
config the client could not describe, a route that needed a destination, and a node that was
silent when paid. `init` writes around all four, and the rest of this page explains each file
it wrote so you can change it.

Only `--devnet` exists. A mainnet node settles real money in a token you choose, and every
value in that block is yours to read off the chain ([step 3](get-paid.md)). Flags:
`--dir <path>` (default `.`), `--address <g.ilp.address>` (default `g.devnet.<six hex digits>`,
also the one route's prefix), `--image <ref>` (what `compose.yml` runs; pin a release handle
for a node you keep). A directory that already holds a `config/connector.toml` is refused by
name, because rotating the signer key would change the node's identity under every peer.

## With Docker

The published image is how this is meant to be deployed. It runs as uid `10001`
and creates `/app/state` owned by that uid, so a fresh named volume inherits the
ownership.

Pin a **release handle** — `rust-<handle>`, a UTC date and that day's ordinal.
Handles are immutable and never moved. The image reads every key file you mount
as uid `10001`, so hand the key to that uid when you make it: a key you own
with mode `600` is unreadable to the container, which restarts forever with
`failed to read signer key_file … Permission denied`. The same applies to every
settlement key in step 3.

```bash
docker pull ghcr.io/toon-protocol/connector:rust-2026.09.29.1
mkdir -p node/config node/data && cd node
openssl rand -hex 32 > data/signer.key
chmod 600 data/signer.key && sudo chown 10001 data/signer.key
```

`rust-sha-<short-sha>` pins an exact commit and is equally immutable. `rust-main`
floats on every green build of `main` — fine for a scratch trial, never for a
deployment. [`deploy/connector-rust/README.md`](../../deploy/connector-rust/README.md)
has the full tag table.

> [!NOTE]
> **The published image is `linux/amd64` only.** On Apple Silicon (or any other
> arm64 host), pull and run it under emulation:
>
> ```bash
> docker pull --platform linux/amd64 ghcr.io/toon-protocol/connector:rust-2026.09.29.1
> ```
>
> and add `platform: linux/amd64` next to `image:` on the `connector` service in
> `compose.yml` below.

`compose.yml`:

```yaml
services:
  connector:
    image: ghcr.io/toon-protocol/connector:rust-2026.09.29.1
    command: ['/app/config/connector.toml']
    volumes:
      - ./config/connector.toml:/app/config/connector.toml:ro
      - ./data:/app/data:ro
      # A NAMED volume, not a bind mount. A host bind mount arrives root-owned
      # and the connector refuses to start.
      - connector-state:/app/state
    ports:
      # Loopback to begin with. The client edge is the paid surface; put a
      # TLS-terminating reverse proxy in front of it before it faces the world.
      - '127.0.0.1:3000:3000'
    restart: unless-stopped
    healthcheck:
      # Free and unauthenticated. Answering it means the config loaded, every
      # settlement backend connected, and the router is serving. `docker ps`
      # showing "Up" proves none of that.
      test: ['CMD', 'wget', '-qO-', 'http://127.0.0.1:3000/ilp/identity']
      interval: 10s
      timeout: 3s
      retries: 5

  quotes:
    image: your-quotes-app:latest
  search:
    image: your-search-app:latest

volumes:
  connector-state:
```

```bash
docker compose up -d
```

## From source

For changing the connector rather than running it. Rust stable:

```bash
git clone https://github.com/toon-protocol/connector.git
cd connector && cargo build --workspace
./target/debug/connector path/to/connector.toml
```

Paths in the config are then yours, not the image's: `state_dir = "./state"`,
`key_file = "./signer.key"`. [`CONTRIBUTING.md`](../../CONTRIBUTING.md) has the test
gate and the chain binaries it needs.

## The config

```toml
client_edge_addr = "0.0.0.0:3000"
state_dir        = "/app/state"

[signer]
key_file = "/app/data/signer.key"   # 32 raw bytes, or 64 hex characters

# One route per thing you serve. Longest matching prefix wins, so a more
# specific prefix can sit beneath a broader one and take precedence.
#
# `price` is a whole number of the SMALLEST UNIT of the token you settle in
# (step 3) — the way a card terminal counts in cents, never in dollars. How
# many of those units make one token is the token's `decimals`. USDC has 6,
# so 1,000,000 units are one USDC, and:
#
#     price = 1000        0.001 USDC   a tenth of a cent
#     price = 100000      0.10  USDC   ten cents
#     price = 1000000     1.00  USDC   one dollar
#
# A wallet or a dashboard shows the whole-token figure. This file never does.
[[routes]]
prefix      = "g.example.quotes"
handler_url = "http://quotes:8080/"
price       = 1000                       # 0.001 USDC per request

[[routes]]
prefix      = "g.example.search"
handler_url = "http://search:8080/"
price       = 2500                       # 0.0025 USDC

# Same app, deeper prefix, different price. This wins over g.example.search
# for g.example.search.bulk because it matches more labels.
[[routes]]
prefix      = "g.example.search.bulk"
handler_url = "http://search:8080/bulk/"
price       = 10000                      # 0.01 USDC, a cent

# If your app's own costs go up with the size of what it is handed — storage,
# uploads, anything you pay an upstream by the byte for — price it by size
# instead of picking one number and losing money at one end of the range:
#
#     base     what every request pays, whatever it carries
#     per_kib  added for each started kibibyte of the request's payload
#
# The route's PRICE is this schedule. What one packet actually costs under it
# is that packet's CHARGE: `base + per_kib × ceil(payload_size / 1024)`. Both
# figures are published, so a caller can work out a charge before sending.
# Leave `price` a plain number when one number is right — that is still what
# most routes want.
[[routes]]
prefix      = "g.example.store"
handler_url = "http://store:8080/"
price       = { base = 1000, per_kib = 30 }   # 0.001 USDC + 0.00003 per KiB
```

Then ask the node what it is:

```bash
curl http://localhost:3000/ilp
```

That free, unauthenticated `GET` returns the node's **self-description** — its
addresses, endpoints, identity key and settlement facts. A connector answers; it
never announces. It is also the whole of what another operator needs to peer with
you, once [`[node]` and `peer_expose`](peering.md#being-peerable) are set — this minimal
config's own self-description has no endpoints and nothing to dial.

Three things this minimal node cannot do yet, and where each is fixed:

- **It cannot be paid, or even described by the client.** With no `[settlement]`
  table, `toon-client describe` and `price` refuse it with "publishes no
  settlement chains at all". [Step 3](get-paid.md) is eight lines on devnet.
- **A caller must name a route.** With no destination, the client addresses the
  node's own ILP address from `GET /ilp`, which works only when that address is
  also the prefix of a priced route (the devnet store's `g.toon.store` is). Set
  `[node] addresses` to your route's prefix, or expect callers to pass it.
- **You cannot see that you were paid.** At the default log level the node
  writes one line when a channel is admitted and nothing per voucher. The
  [operator surface](operator-surface.md) — `GET /claims` and the dashboard —
  is how an operator watches the money.

> [!IMPORTANT]
> **One TOML file, read once, immutable for the process lifetime.** There is no
> environment-variable layer — `CONFIG_FILE` and friends do nothing, and the only
> variable read is `RUST_LOG`. An unknown key is a hard load failure and a removed
> key is refused **by name**, so a stale config says so at boot instead of quietly
> doing nothing.
>
> **Every key is a path, never a value.** No inline keys, no mnemonic, nowhere to
> smuggle one through.
>
> **`state_dir` is where this node records which claims it has already been
> paid.** In a container it must be a mounted volume; a watermark that dies with
> the container hands every payer their spent claims back as free service.
>
> One directory belongs to one process. A node locks `state_dir` before it opens
> anything there, and a second connector started over the same directory refuses
> to start, naming it. The lock is the operating system's, so a node that crashed
> leaves nothing to clean up.
