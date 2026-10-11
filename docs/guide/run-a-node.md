# 1. Run a node

The connector is one static binary that reads one TOML file. Two ways to get it.

## With Docker

The published image is how this is meant to be deployed. It runs as uid `10001`
and creates `/app/state` owned by that uid, so a fresh named volume inherits the
ownership.

Pin a **release handle** — `rust-<handle>`, a UTC date and that day's ordinal.
Handles are immutable and never moved:

```bash
docker pull ghcr.io/toon-protocol/connector:rust-2026.09.29.1
mkdir -p node/config node/data && cd node
openssl rand -hex 32 > data/signer.key && chmod 600 data/signer.key
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
