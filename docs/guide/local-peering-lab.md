# Try it locally: two connectors, one packet, one chain

The lab for [4. Peer with another node](peering.md).

Everything above, on a disposable chain, in about five minutes. Two nodes — **A**
terminates a priced route, **B** peers with A and forwards to it — and one packet
that crosses the peering and comes back fulfilled.

On the host you need **Docker**, **`bash`**, **`curl`**, **`openssl`** and
**`jq`**, and a clone of this repository (the chain's contracts are deployed from
it). You do **not** need Rust, Foundry or the Solana CLI: every chain command
below runs inside a container.

On arm64 — Apple Silicon included — [step 1's amd64 note](run-a-node.md#with-docker) applies
here too, and to more services: pull with `--platform linux/amd64`, add
`platform: linux/amd64` beside `image:` on all three services in the lab's
`compose.yml` below, and pass `--platform linux/amd64` to step 7's `docker run`.

Two paths, used throughout — the lab lives outside the checkout so nothing here
lands in `git status`:

```bash
export REPO=/path/to/connector       # this clone
export LAB=~/peering-lab             # anywhere else
# `rust-main` floats on every green build of main -- fine for a scratch trial
# like this one, never for a deployment (see step 1). Pin a release handle
# instead once one has shipped this lab's config shape; check
# deploy/connector-rust/README.md's tag table for the newest.
export IMAGE=ghcr.io/toon-protocol/connector:rust-main
mkdir -p "$LAB"

docker pull $IMAGE     # ~1 min the first time; everything below assumes it is here
```

## 1. A chain, with the settlement contracts on it

```bash
cd "$REPO"
docker compose --profile evm up -d --wait anvil
```

That starts `anvil` on `127.0.0.1:8545` and seeds it (`infra/anvil/seed.sh`):
x402's `x402BatchSettlement` and its deposit collectors at their canonical
addresses, and Circle's FiatToken v2.2 as USDC. No TOON contract is deployed —
every channel is an x402 channel (ADR 0075), so there is nothing else to seed.
`--wait anvil` is doing two jobs: it blocks until anvil reports **healthy**,
which for this service means the seed has actually landed rather than that the
process started — and naming the service keeps the `evm` profile's other member,
a devnet `faucet` container irrelevant to this walkthrough, out of the way.

Two addresses, deterministic on every fresh anvil, which is what lets them be
written down here:

| What                                         | Address                                                                             |
| -------------------------------------------- | ----------------------------------------------------------------------------------- |
| `x402BatchSettlement`                        | `0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003` — a constant of the binary, not config |
| USDC, FiatToken v2.2, 6 dp (`token_address`) | `0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512`                                        |
| Chain id                                     | `31337`                                                                             |

Anvil's account 0 — `0xf39F…2266`, private key
`0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80` — is the
deployer and holds 10,000 ETH. Account 1 — `0x7099…79C8`, private key
`0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d` — is the
USDC's minter and can mint it to anyone. Both are public test keys; they are not
secrets and nothing outside a local anvil will accept them.

## 2. Keys

```bash
cd "$LAB"
mkdir -p node-a/data node-b/data

for n in node-a node-b; do
  openssl rand -hex 32 > $n/data/signer.key
  openssl rand -hex 32 > $n/data/settlement.key
  openssl rand -hex 32 > $n/data/operator-bearer-token
  openssl rand -hex 32 > $n/data/operator-send.key
done

# The image runs as uid 10001 and mounts these read-only, so it has to be able
# to read them. That is why this lab does NOT chmod 600: these are disposable
# keys on a disposable chain. On a real node the key files are owned by uid
# 10001 instead, and stay unreadable to everyone else.
chmod -R a+rX node-a/data node-b/data
```

`operator-send.key` is the **private** half of an operator write key, and it never
goes on a node. What the node holds is its public half, which the binary that will
do the signing derives for you:

```bash
for n in node-a node-b; do
  docker run --rm -v "$PWD/$n/data:/data:ro" $IMAGE \
    send --operator-key /data/operator-send.key --print-keyid > $n/data/operator-write-keys
done
cat node-b/data/operator-write-keys      # 64 hex characters

# `operator-write-keys` was created after the chmod above, with whatever your
# umask gives it, and both configs name it. Under `umask 077` a node started
# without this line refuses to boot on a file you believe you already fixed.
chmod -R a+rX node-a/data node-b/data
```

## 3. Fund the settlement keys

A peering is **two** x402 channels (ADR 0075): B opens and funds the one that
carries real traffic to A, and A opens a small one of its own back toward B —
unfunded past its opening deposit, since no packet ever flows on it, but
still a real on-chain deposit that has to come from somewhere. So both keys
need a little ETH for gas and a little USDC; B needs much more USDC, since
its channel is the one that gets topped up and actually spent from.

```bash
cd "$REPO"
DEPLOYER=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
MINTER=0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d
USDC=0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512
RPC=http://localhost:8545

# every `cast` below runs inside the anvil container -- nothing is installed here
cast() { docker compose --profile evm exec -T anvil cast "$@"; }

# `cast wallet address` is a local computation and takes no --rpc-url
A_ADDR=$(cast wallet address --private-key 0x$(cat "$LAB"/node-a/data/settlement.key) | tr -d '\r')
B_ADDR=$(cast wallet address --private-key 0x$(cat "$LAB"/node-b/data/settlement.key) | tr -d '\r')
echo "A $A_ADDR   B $B_ADDR"

cast send --rpc-url $RPC --private-key $DEPLOYER --value 10ether  "$A_ADDR"
cast send --rpc-url $RPC --private-key $DEPLOYER --value 100ether "$B_ADDR"
cast send --rpc-url $RPC --private-key $MINTER "$USDC" "mint(address,uint256)" "$A_ADDR" 10000
cast send --rpc-url $RPC --private-key $MINTER "$USDC" "mint(address,uint256)" "$B_ADDR" 1000000000

# check it landed: 100 ETH, and 1000 USDC at 6 decimals
cast balance --rpc-url $RPC "$B_ADDR"
cast call --rpc-url $RPC "$USDC" "balanceOf(address)(uint256)" "$B_ADDR"
```

Fund **before** you boot. A settlement backend that cannot pay looks exactly like
a config error in the logs.

## 4. The two configs

A peering is two one-way channels, so **both** nodes have to be peerable
(`[node]` and `peer_expose`) — each dials the other's self-description once,
whichever end opens which channel. Neither names the peering, or the other's
voucher signer, in its config: both writes happen at runtime, in the next
step.

`$LAB/node-a/connector.toml` — the terminating side:

```toml
client_edge_addr               = "0.0.0.0:3000"
state_dir                      = "/app/state"
peer_expose                    = "http"
peer_allow_plaintext_endpoints = true      # http:// endpoints — local trial only

[node]
addresses     = ["g.lab.a"]
http_endpoint = "http://node-a:3000/ilp"

[signer]
key_file = "/app/data/signer.key"

[[routes]]
prefix      = "g.lab.a.app"
handler_url = "http://stub-app:3100/"
price       = 1000

[operator]
bearer_token_file = "/app/data/operator-bearer-token"
write_keys_file   = "/app/data/operator-write-keys"

[settlement.evm]
rpc_url               = "http://anvil:8545"
token_address         = "0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512"
decimals              = 6
asset_eip712_name     = "USDC"
asset_eip712_version  = "2"

[settlement.evm.key]
key_file = "/app/data/settlement.key"
```

`$LAB/node-b/connector.toml` — the forwarding side. It also needs `[node]`
and `peer_expose` now: A's own `POST /peers` (below) dials B's self-description
to open A's small reverse channel and bind B's voucher signer to it:

```toml
client_edge_addr               = "0.0.0.0:3000"
state_dir                      = "/app/state"
peer_expose                    = "http"
peer_allow_plaintext_endpoints = true

[node]
addresses     = ["g.lab.b"]
http_endpoint = "http://node-b:3000/ilp"

[signer]
key_file = "/app/data/signer.key"

[operator]
bearer_token_file = "/app/data/operator-bearer-token"
write_keys_file   = "/app/data/operator-write-keys"

[settlement.evm]
rpc_url               = "http://anvil:8545"
token_address         = "0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512"
decimals              = 6
asset_eip712_name     = "USDC"
asset_eip712_version  = "2"

[settlement.evm.key]
key_file = "/app/data/settlement.key"
```

## 5. Bring the nodes up

`$LAB/compose.yml`. `stub-app` is the connector image's second binary — it
answers `POST /` and holds no secret, which is the point: it contributes nothing
to the packet's fulfilment. Both nodes join the network anvil is already on:

```yaml
services:
  stub-app:
    image: ghcr.io/toon-protocol/connector:rust-main
    entrypoint: ['/usr/local/bin/stub-app']
    command: ['0.0.0.0:3100']

  node-a:
    image: ghcr.io/toon-protocol/connector:rust-main
    command: ['/app/config/connector.toml']
    volumes:
      - ./node-a/connector.toml:/app/config/connector.toml:ro
      - ./node-a/data:/app/data:ro
      - a-state:/app/state
    ports: ['127.0.0.1:3010:3000']
    # Same probe as step 1's compose.yml, and for the same reason: "Up" says a
    # container exists, not that a node is serving. The next thing you do is
    # curl it, so the difference is the whole race.
    healthcheck:
      test: ['CMD', 'wget', '-qO-', 'http://127.0.0.1:3000/ilp/identity']
      interval: 2s
      timeout: 3s
      retries: 15

  node-b:
    image: ghcr.io/toon-protocol/connector:rust-main
    command: ['/app/config/connector.toml']
    volumes:
      - ./node-b/connector.toml:/app/config/connector.toml:ro
      - ./node-b/data:/app/data:ro
      - b-state:/app/state
    ports: ['127.0.0.1:3011:3000']
    depends_on:
      node-a: { condition: service_healthy }
    healthcheck:
      test: ['CMD', 'wget', '-qO-', 'http://127.0.0.1:3000/ilp/identity']
      interval: 2s
      timeout: 3s
      retries: 15

volumes:
  a-state:
  b-state:

networks:
  default:
    name: connector_default # the network `docker compose --profile evm` made
    external: true
```

That network name does not depend on what you called your clone. Compose usually
derives a project name from the directory, but this repository's
`docker-compose.yml` pins `name: connector`, so its network is
`connector_default` whatever the checkout is called — which is what makes it safe
to write down here, and why step 7's `docker run --network connector_default`
works too.

The two `connector.toml` files sit outside `data/`, so step 2's `chmod` never
reached them, and uid 10001 has to read them too:

```bash
chmod a+r "$LAB"/node-a/connector.toml "$LAB"/node-b/connector.toml
cd "$LAB" && docker compose up -d --wait
```

`--wait` returns only once both nodes answer their healthcheck, so when it comes
back they are serving and the `curl` below cannot race them.

If it returns non-zero, or a node exits immediately, `docker compose logs node-a`
says why in one line — a config that does not load, or a chain that disagreed with
it, stops the node deliberately rather than serving something half-configured.

Now read A the way B is about to:

```bash
curl -s http://127.0.0.1:3010/ilp | jq
```

`"peerCarriages": ["http"]`, an `httpEndpoint`, an `edgeIdentity` and one
`batchSettlements` entry on `eip155:31337` means A is peerable — and B's own
document, at `http://127.0.0.1:3011/ilp`, now shows the same three, since B
also has to be dialed for A's reverse channel in the next step.

## 6. Peer them

Four signed writes — two against each node — because a peering is two
channels and each node opens its own. `sign-write.sh` lives in this
repository:

```bash
SW="$REPO"/docs/operators/sign-write.sh
KA="$LAB"/node-a/data/operator-send.key
KB="$LAB"/node-b/data/operator-send.key
A=http://127.0.0.1:3010
B=http://127.0.0.1:3011

# 1. A's own reverse leg first, so B's first voucher already arrives on a
#    bound channel: A reads B's self-description and opens a small channel
#    of its own toward B, with `fee` and `max_packet_amount` both 0 because
#    nothing is ever forwarded on it.
$SW -k $KA -X POST -p /peers -u $A \
  -b '{"id":"b","url":"http://node-b:3000/ilp","fee":0,"max_packet_amount":0,"deposit":1000}'

# 2. B's own leg. B reads A's self-description and opens the channel that
#    actually carries traffic, with an opening deposit.
$SW -k $KB -X POST -p /peers -u $B \
  -b '{"id":"a","url":"http://node-a:3000/ilp","fee":100,"max_packet_amount":5000,"deposit":5000}'
```

B's answer to its own write:

```json
{
  "id": "a",
  "fee": 100,
  "max_packet_amount": 5000,
  "source": "runtime",
  "channel": { "id": "0x…", "status": "created", "chain": "evm", "network": "eip155:31337" }
}
```

The channel id is the one value you carry between commands. Lift it rather than
retyping 66 hex characters — `sign-write.sh` prints its three headers first, so
the response body is the last line:

```bash
CH=$($SW -k $KB -X POST -p /peers -u $B \
      -b '{"id":"a","url":"http://node-a:3000/ilp","fee":100,"max_packet_amount":5000,"deposit":5000}' \
     | tail -1 | jq -r .channel.id)
echo "$CH"
```

Re-running `POST /peers` like this is safe: the peering is already established, so
this second call finds the same channel, spends no `deposit` again, and answers
`"status":"found"`.

```bash
# 3. Top B's channel up — its own collateral behind its own vouchers. Ten
#    packets' worth: every packet spends collateral INCLUDING one that comes
#    back REJECT, so leave yourself room to experiment. `fund` takes an
#    INCREMENT, so this adds 5000 to the 5000 already deposited, for 10000
#    total. Running out reads as "T00 ... has 0 base units of headroom left".
$SW -k $KB -X POST -p /channels/$CH/fund -u $B -b '{"amount":5000}'

# 4. Route through it. 1100 in, 100 kept, 1000 forwarded — exactly A's price.
$SW -k $KB -X POST -p /routes/peers -u $B \
  -b '{"prefix":"g.lab.a","peer_id":"a","price":1100}'
```

Read the table back, with the bearer token this time — reads need no signature:

```bash
curl -s -H "Authorization: Bearer $(cat "$LAB"/node-b/data/operator-bearer-token)" \
  $B/routes/peers | jq
```

## 7. Probe the path, then send a packet across it

Learn what the path costs before paying it. `connector probe` takes what `send`
takes to form a packet and sends it at `--amount 0` (the default), so a priced
route, or a hop charging a fee on the way, refuses it and the refusal states the
cost ([ADR 0011](../adr/0011-rejects-accumulate-fees-and-probes-discover-cost.md)).
A probe is an ordinary packet through the same `POST /packets`; at amount 0
nothing is paid. Give it the payload you mean to send, because a route may
price by size:

```bash
echo '{"hello":"from a paid packet"}' > "$LAB"/node-b/data/payload.json
chmod a+r "$LAB"/node-b/data/payload.json

docker run --rm --network connector_default \
  -v "$LAB/node-b/data:/data:ro" \
  $IMAGE probe \
    --operator     http://node-b:3000 \
    --operator-key /data/operator-send.key \
    --to           g.lab.a.app \
    --seal-to      http://node-a:3000/ilp \
    --body         /data/payload.json
```

It prints one of three answers and exits zero on each: `COST <n>`, the whole
path's cost for a packet this size; `PARTIAL COST <n>`, an `R01` that stopped at
a hop it could not pay, where `<n>` is the amount to carry past that hop (probe
again with `--amount <n>` to read on); or `DELIVERED`, when `--amount` already
covered the path and the packet was sent and paid for. A path that does not
exist or did not answer (no route, a timeout, an unreachable peer) exits
non-zero. `--json` prints the code, message and `accumulatedCost` as one object.

Then send it, with the amount the probe named. `connector send` is the binary's
verb for that: it forms a real packet, seals it, signs the write and reports the
outcome:

```bash
docker run --rm --network connector_default \
  -v "$LAB/node-b/data:/data:ro" \
  $IMAGE send \
    --operator     http://node-b:3000 \
    --operator-key /data/operator-send.key \
    --to           g.lab.a.app \
    --seal-to      http://node-a:3000/ilp \
    --target       / \
    --amount       1000 \
    --body         /data/payload.json \
    --expect-fulfill
```

Three flags decide whether this works, and each fails quietly if you get it
wrong:

- **`--operator` is B, `--seal-to` is A.** You hand the packet to B, but the
  payload is sealed to the node that **terminates** it
  ([ADR 0018](../adr/0018-a-payload-is-sealed-to-the-terminating-connector.md)),
  and B cannot open it. Sealing to B instead comes back as a
  `REJECT F01 -- gift wrap could not be opened: …`, which is the one mistake
  here that announces itself.
- **`--seal-to` takes a self-description URL**, ending in `/ilp`, not an origin.
- **`--amount` is the path's cost to you**, not the route's price: the
  terminating price plus the fee of every hop _after_ the node you hand it to.
  That node charges its operator's own packet nothing, so here there is no
  other hop and the cost is 1000. One unit short and the packet comes
  back `REJECT F03 -- claim rejected: advances value by 999, less than this
route's price of 1000`.

`--expect-fulfill` is what makes this a test rather than a report: without it a
`REJECT` is printed and the process still exits 0.

## 8. Prove it was actually paid for

A fulfilment does **not** prove the peering was paid. The verdict on a peer's claim
travels back beside the packet's own answer and never gates it, so a peering whose
every claim was refused still fulfils — `--expect-fulfill` would stay green over a
peering carrying your traffic for free. The evidence is on the payee, in A's own
claim journal:

```bash
curl -s -H "Authorization: Bearer $(cat "$LAB"/node-a/data/operator-bearer-token)" \
  http://127.0.0.1:3010/claims | jq
```

```json
[
  {
    "peer_id": null,
    "channel_id": "evm:0x…",
    "direction": "inbound",
    "nonce": 0,
    "cumulative_amount": 1000,
    "pending": false,
    "book": "client",
    "scheme": "batch-settlement"
  }
]
```

`cumulative_amount` is 1000, not 1100: B collected 1100 and kept its 100 fee.
Send a second packet and the cumulative becomes 2000. A voucher has no nonce
(`nonce` is always `0`): a voucher that merely repeats the same cumulative
advances nothing and buys nothing, so **the watermark is the thing to watch**,
not the claim count.

## 9. Tear it down

```bash
cd "$LAB"  && docker compose down -v
cd "$REPO" && docker compose --profile evm down -v
```

Both chains and both state volumes are disposable, and `-v` matters: a claim
journal left behind satisfies the next run's money check without the next run
paying for anything.
