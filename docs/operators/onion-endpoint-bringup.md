# Bringing a connector up behind an onion endpoint

> **What this buys, and what it does not.** An onion endpoint gives a node **inbound reachability**
> without a public IP address, a DNS name or a TLS certificate. It hides **where a node is
> reachable**, and nothing else. Every claim names an on-chain channel and address, and every
> operator write is RFC 9421-signed under a keyid: who paid whom is on a public chain either way.
> This runbook makes no anonymity claim beyond that sentence, and neither does
> [ADR 0070](../adr/0070-an-onion-address-is-a-host-not-a-carriage.md).

Operator runbook for [ADR 0070](../adr/0070-an-onion-address-is-a-host-not-a-carriage.md), built in
[#1273](https://github.com/toon-protocol/connector/issues/1273). It covers the two things that are
**not** the connector's to do — running the daemon that generates the address, and persisting the
directory that address lives in — plus the four config values an operator writes once the daemon has
produced one.

Modeled on [`btp-peer-transport-bringup.md`](btp-peer-transport-bringup.md)'s "Order" and "Gates"
sections, which is where the peering surface itself is documented. Nothing here replaces it: an
onion peering is an ordinary peering whose endpoint happens to have a hidden-service host, so every gate
in that runbook still applies unchanged. What follows is only the difference.

## Which TLD your daemon speaks — `.onion` or `.anyone`

**Check this before anything else in this runbook.** `anon` renamed the TLD it publishes and routes
between v0.4.9.7 and v0.4.10.2, and the rename is total in both directions
([issue #1284](https://github.com/toon-protocol/connector/issues/1284)):

| Release       | Writes into `hostname` | Routes    | Contains the other spelling      |
| ------------- | ---------------------- | --------- | -------------------------------- |
| **v0.4.9.7**  | `<56-base32>.onion`    | `.onion`  | no `.anyone` at all              |
| **v0.4.10.2** | `<56-base32>.anyone`   | `.anyone` | **zero** occurrences of `.onion` |

A daemon refuses the spelling it does not know — a `.onion` name offered to v0.4.10.2's SOCKS port
comes back host-unreachable, and the reverse holds too. So the spelling is **the daemon's**, not
yours: write down whatever `hostname` actually says.

**The connector accepts both** (ADR 0070 as amended). One suffix rule, `is_onion_endpoint`, and it
matches either — so a config is never wrong here for naming the TLD your own daemon generated. What
it cannot do is bridge the two: a node published by one release is unreachable through the other's
proxy, whatever any config says, because no circuit exists between them. **You and every
counterparty you peer with must be on the same side of the rename**, and the check is one command on
each box:

```sh
anon --version                                     # or: docker compose exec anon anon --version
```

`local/anon-image/` builds v0.4.10.2, which is the current release and the one `toon-client` pins;
ghcr publishes no image for it, which is why that Dockerfile exists. The examples below are spelled
`.anyone` for that reason; substitute `.onion` throughout if you are running the older daemon.

## No box on this fleet runs one

Neither devnet box has an onion endpoint, and nothing in this repository deploys one. The rehearsal
that exists is [`local/onion`](../../local/README.md) — two connector images, two real `anon`
sidecars, each connector on its own docker network with **no route between them**, so that a
fulfilled packet is evidence of a circuit rather than evidence of a docker network. It is
deliberately not on the CI gate (`local/README.md` says why, at length; do not "fix" its absence).

Run it before you run this:

```sh
make local-verify LOCAL_TOPOLOGY=onion
```

It is the same shape as the deployment below, with `local/keys.sh onion` playing the operator for
the one manual step — it starts the sidecars, reads the generated address out of the daemon's
`hostname` file, and renders it into the configs compose mounts. On a real box, that copying is
yours.

## Carriage, and what is unchanged

An onion address is a **host**. It is not a scheme, not a carriage and not a transport. Both
carriages ADR 0027 settled on ride it unchanged:

| Carriage      | Clearnet endpoint        | Onion endpoint               |
| ------------- | ------------------------ | ---------------------------- |
| BTP           | `wss://host:443/ilp/btp` | `ws://<addr>.anyone/ilp/btp` |
| ILP-over-HTTP | `https://host/ilp`       | `http://<addr>.anyone/ilp`   |

`peer_expose`, a route's `transport` policy, the greeting's `requiredTransport` and
`peer-carriage-spec.md` §11's spellings are all untouched, and there is no `PeerCarriage::Onion` —
the record's own falsifier is that there is not. The client edge is served over the same onion
service as the peering, because it is the same listener: one hidden service, both surfaces, on the
paths `client_edge_addr` already serves.

**The plaintext schemes here need no opt-in.** `ws://` and `http://` select a carriage at a host
ending in `.onion` or `.anyone` on their own, independently of `peer_allow_plaintext_endpoints`, which keeps its
existing meaning and its existing scope as a loopback-and-test switch that must never be set on a
deployed node. The exemption is narrow on purpose: a v3 onion address **is** the ed25519 public key
the circuit is authenticated to, so an endpoint at that host authenticates itself, and ADR 0004's
requirement is satisfied by a different mechanism rather than waived. A host that merely _contains_
either suffix — `onion.example`, `notreally.onion.example`, `anyone.example` — is still refused as
plaintext, by name (`ConfigError::PeerEndpointScheme`): these are suffixes, not substrings.

## Who does what

| Step                                        |  Repo-side (PR, reviewable)  | Human-only (the box, the daemon, the address) |
| ------------------------------------------- | :--------------------------: | :-------------------------------------------: |
| 1. Run the daemon as a sidecar              | ✅ compose service, `anonrc` |             deploys and starts it             |
| 2. `AgreeToTerms 1`                         | ✅ one line in the `anonrc`  |                                               |
| 3. Persist `HiddenServiceDir`               |      ✅ a named volume       |        verifies it survived a restart         |
| 4. Read the generated address               |                              |     ✅ reads `HiddenServiceDir/hostname`      |
| 5. Write it into `[node]`                   |         ✅ config PR         |               deploys, restarts               |
| 6. `socks_proxy` + peer `endpoint` (dialer) |         ✅ config PR         |               deploys, restarts               |
| 7. Rehearse with `connector send`           |                              |    ✅ runs it from a box that has a proxy     |

Step 4 is the only one nothing in this repository can do for you, and that is a decision (ADR 0070
decision 7), not a gap — see "The operator writes the address down", below.

## Preconditions

- A node that already **serves**: `client_edge_addr` bound, `state_dir` on a mounted volume, keys
  in place. `deploy/connector-rust/README.md` is that path; this runbook changes where the node is
  reachable, not what it is.
- The peering surface configured on both sides as `btp-peer-transport-bringup.md` describes:
  `[[peers]]`, `[[peer_channels]]`, and `[[pay_channels]]` on the side that pays. **Since ADR 0060
  a peering is proven by a verified claim**, so the channel rows are the whole of peer
  authentication — the circuit authenticates the endpoint, never the peer.
- An onion-routing daemon available to run **beside** the connector, not inside it. Anyone
  Protocol's `anon` is a Tor fork — binary `anon`, config `anonrc`, SOCKS on 9050 — and plain Tor
  works identically: the connector's whole surface is a `socks5h://` URL and a hidden-service
  host, and nothing in it is specific to either. Which `anon` release you run decides the TLD —
  see the section above.
- A settlement RPC endpoint and an app `handler_url` the node can reach **directly**. Neither is
  proxied (below), so an onion endpoint does not relieve the box of ordinary outbound network
  access.

## Order — daemon through peering, in order

### 1. Run the daemon as a sidecar

`anon` is a C daemon and the connector is one Rust binary. The relationship is the one this
repository already has with `anvil` and `solana-test-validator`: infrastructure the node talks to,
never something it contains (ADR 0070 decision 8). Give it its own container, its own `anonrc` and
its own volume.

The daemon serving an onion service needs no SOCKS port at all — a node that is only dialed dials
nothing. The daemon on the **dialing** side needs a SOCKS port and publishes no service. A node that
does both runs both, or one daemon configured for both; the connector cannot tell the difference,
because all it holds is one proxy URL.

### 2. `AgreeToTerms 1`, or the daemon does not start

`anon` refuses to run until its terms are accepted, and in a non-interactive container it **fails
fast** rather than prompting:

```
User has not agreed to the terms and conditions. Exiting.
```

Put `AgreeToTerms 1` in the `anonrc` before the first boot. It is the single most common reason a
first bring-up produces a container that exits immediately with a connector beside it that looks
healthy and is unreachable. `local/onion/anonrc-a` and `anonrc-b` are worked examples.

Two adjacent traps the local topology already hit, worth inheriting: a read-only `anonrc` mount
needs an explicit `Nickname`, because the image's entrypoint appends one when it finds none and
cannot write to a read-only file; and a container that is `Up` is not a container that has a
circuit, so gate on `Bootstrapped 100%` in the log rather than on the container's state.

### 2b. `HiddenServicePort`'s target is resolved when the config is **parsed**

`anon` resolves the address in `HiddenServicePort` while reading its `anonrc`, not when a stream
arrives. A target it cannot resolve **aborts the daemon before it runs**, and it aborts by crashing:

```
[warn] Unparseable address in hidden service port configuration.
free(): invalid size
```

That bites exactly where this runbook puts you. The connector's config names an address only the
daemon can generate, so the daemon has to start first — and if `HiddenServicePort` names the
connector by a container name that does not exist yet, the daemon dies on the name rather than
waiting for it. Point it at something that resolves at parse time: a **fixed IP** on the container
network (`local/anyone/anonrc-b` does this, and its compose pins the connector to that address), or
a host that is already up.

Belongs beside `AgreeToTerms 1` in your head: both are configuration faults that kill the daemon
before it can say anything useful about itself, leaving a connector beside it that looks healthy and
is unreachable.

### 3. `HiddenServiceDir` must be on a persisted volume — the same rule as `state_dir`

**This is the failure that costs the most and announces itself the least.** The daemon generates the
onion address into `HiddenServiceDir` on first start and keeps the private key beside it. If that
directory is not on a persisted volume, the daemon generates a **new** address on every restart, and
every counterparty's configuration goes stale **silently**: their `endpoint` still parses, still
selects a carriage, still loads — and dials an address that no longer exists.

That is precisely `state_dir`'s failure mode one indirection out, so keep the two in the same place
in your head and in the same review:

| Directory          | Whose           | Unpersisted, you lose                                       | How it fails                                                               |
| ------------------ | --------------- | ----------------------------------------------------------- | -------------------------------------------------------------------------- |
| `state_dir`        | the connector's | every channel's replay watermark                            | a restarted node accepts a nonce it already spent — free service, silently |
| `HiddenServiceDir` | the daemon's    | the node's address, and with it every counterparty's config | every peer dials an address that is gone — silently, on their side         |

Use a **named volume**, not a path in the writable layer and not a host bind mount you have to
`chown` — the same argument that puts `/app/state` on one. Back it up if losing the address would
cost more than telling every counterparty a new one.

Note what is _not_ being asked: the key is never committed and never leaves that volume. A fixed
onion key in a repository is an address anyone who cloned it can impersonate, and
`tools/ci/check-tracked-secrets.sh` would refuse it anyway.

### 4. Read the address out of the daemon

```sh
docker compose exec -T anon cat /var/lib/anon/hidden_service/hostname
```

A v3 address is 56 characters of the base32 alphabet followed by the TLD **that daemon** speaks —
`.anyone` on v0.4.10.2, `.onion` on v0.4.9.7 (see "Which TLD your daemon speaks" above). Anything
shorter, or an empty file, means the daemon has not finished publishing its descriptor yet — wait
for `Bootstrapped 100%` and read again. Copy what the file says; do not translate it.

### 5. The operator writes the address down

The connector **never reads that file** and **never speaks the daemon's control protocol** (ADR 0070
decision 7). It is ADR 0050's shape exactly — a fact no process can introspect about itself — and it
is the same reason `[node]` exists at all. Copy the address into two places, by hand:

**On the onion node**, into `[node]`. No new key: a hidden-service URL is a legal value for the
`http_endpoint` and `btp_endpoint` this section already has (decision 6), and issue #1220's rule
holds unmodified — `btp_endpoint` is required exactly when `peer_expose` opens a BTP listener, and
`http_endpoint` whenever anything is exposed, because a peer pays this node by asking its client
edge for claim-state over HTTP whichever carriage a packet rides.

```toml
[node]
addresses     = ["g.example.onionnode"]
http_endpoint = "http://<56-char-address>.anyone/ilp"
btp_endpoint  = "ws://<56-char-address>.anyone/ilp/btp"
```

**On the dialing node**, into the peer's `endpoint` — and into the `[[pay_channels]]` row's
`client_edge_url`, if this side pays. Step 6.

### 6. The dialing side: one proxy key, and the endpoints

`socks_proxy` is a **root-level** key, so — like `peer_expose` — it must be written **before the
first `[section]` header**: TOML has no way to write a root-table key once a table header has
appeared earlier in the file.

```toml
socks_proxy = "socks5h://anon:9050"
```

The `h` is not a preference and is refused by name without it (`ConfigError::SocksProxyScheme`): a
`socks5://` proxy resolves the hostname **locally** and dials the address it gets back, and no local
resolver resolves a hidden-service name. A node that accepted one would come up clean and then fail every
onion dial at dial time, for a reason nothing in its log explains. A value that parses but names no
host — `socks5h://`, or a `socks5h:9050` that looks like a `host:port` and is not one — is refused
the same way (`ConfigError::SocksProxyNoHost`), because `socks5h` is not a _special_ URL scheme and
both of those parse.

**There is nothing else to configure, and that is deliberate.** Which dials use the proxy is read
off each endpoint's own **host**: a host ending in `.onion` or `.anyone` goes through the proxy, everything else
goes direct. There is no per-peer `proxy` key and no all-outbound mode, because the address already
carries the answer and a second place to state it is how a peering ends up dialed the wrong way.
One rule, one implementation (`connector_config::is_onion_endpoint`), asked in four places:

- the peering's `endpoint`;
- the `[[pay_channels]]` row's `client_edge_url`;
- the self-description a **runtime** peering is read from (`POST /peers`, ADR 0058);
- the SOCKS5 dial selection on both carriages.

```toml
[[peers]]
id       = "onionpeer"
endpoint = "ws://<56-char-address>.anyone/ilp/btp"  # or http://…/ilp for ILP-over-HTTP

[[pay_channels]]
peer_id         = "onionpeer"
client_edge_url = "http://<56-char-address>.anyone/ilp"
# channel_id, chain_id and token_network as they would be on any peering
```

The `client_edge_url` is easy to forget and fails confusingly. A covering payer asks the payee where
its claims stand on **every** covered PREPARE (issue #1102), so if that URL is a clearnet one, or
missing, the packet is refused for want of a covering claim long before the carriage is asked to
carry anything. Both surfaces are behind the same onion address because they are the same listener.

A node with **no** `socks_proxy` dials everything direct, which is every existing config in this
repository and the right default. On such a node an onion dial fails as an ordinary unreachable
endpoint, and `POST /peers` at an onion URL is refused before any request is made, naming
`socks_proxy` as what is missing.

### 7. Rehearse from outside, with `connector send`

`connector send` loads no configuration file, so it takes the proxy as a flag (decision 5):

```sh
connector send --socks-proxy socks5h://127.0.0.1:9050 \
  --operator http://<addr>.anyone/ilp \
  --seal-to  http://<addr>.anyone/ilp \
  --expect-fulfill …
```

The flag applies the same host-selected rule to **both** URLs it dials — `--operator` and
`--seal-to` — so a node whose terminating peer is also onion-only is probeable in one invocation. No
environment variable is consulted for it: this repository has no environment-variable layer (ADR
0009), and a value that changes where a signed operator write goes should not arrive invisibly.
`--dry-run` fetches the far side's self-description over the circuit and sends no packet, which is
the cheapest way to ask "has a rendezvous happened yet" while circuits are still building.

## Gates — in order, and do not reorder (c)

- **(a) The daemon has a circuit.** `Bootstrapped 100%` in its log, and
  `HiddenServiceDir/hostname` holds a 56-character v3 address. A container that is `Up` is not a
  container that has a circuit.
- **(b) The address survives a restart.** Restart the daemon container and read `hostname` again: it
  must be **the same address**. If it is not, `HiddenServiceDir` is not persisted — stop here, fix
  step 3, and tell no counterparty the address you read before now.
- **(c) The peering establishes, and it charges.** Every gate in
  [`btp-peer-transport-bringup.md`](btp-peer-transport-bringup.md) applies unchanged, and gate (c)
  there — a paid write end to end with **no free-write path** — is the one that matters most here
  for the same reason it matters there: a claim's verdict rides back in `Toon-Claim-Ack` and never
  gates the packet, so a fulfilled packet alone would go green over a peering carrying traffic for
  free. Read the payee's own claim journal and confirm its watermark advanced.
- **(d) The dial actually took the circuit.** A SOCKS dial that silently fell back to a direct
  connection would fulfil exactly as happily. On a real box the honest check is the negative one:
  the onion node publishes **no** clearnet endpoint and no host port, so there is nothing else the
  dialer could have reached. If your onion node is also reachable on the clearnet, this gate proves
  nothing and you are rehearsing, not deploying.

## What is not proxied, and what that leaves linkable

**Settlement RPC and the app's `handler_url` dial direct** (ADR 0070 decision 4). This is a named
limitation of that record, not an oversight: routing settlement through a circuit is a separate
decision with its own evidence to gather, because circuit latency interacts with confirmation
semantics and nonce handling on both settlement backends.

The consequence, stated plainly so that you can decide about it rather than discover it:

- A node reaching a public RPC provider does so **from its real address**.
- That same provider sees **the transactions the node submits**.
- So an observer positioned there can link the operator's network location to their on-chain
  identity. Running the ILP wire over onion does not prevent it.

If that linkage matters to you, the levers are outside this connector — your own RPC node, or a
transport for RPC arranged at the host — and none of them is configured here.

## What an onion endpoint hides

**Where a node is reachable, and nothing else.** Every claim names an on-chain channel and address,
and every operator write is signed under a keyid: who paid whom is on a public chain either way.
Make no claim beyond that sentence to anyone deciding whether to run one — the settlement layer
contradicts every larger claim, and the name "hidden service" invites exactly the reading this
paragraph refuses.

## An onion node is invisible to the clearnet fleet

Its `GET /ilp` publishes a hidden-service URL, and a peer without a proxy cannot dial it. **This is
expected, not a defect.** `CONTEXT.md` already holds that reachability is the only registry — there
is no directory a node is missing from — but the statement here is stronger than "unreachable
today": the existing devnet boxes cannot reach such a node **at all**, because neither of them
configures a `socks_proxy`.

So an onion node peers with counterparties who have arranged a proxy, and with nobody else. Plan the
peering as a two-sided change: your address in their config, their `socks_proxy` in their file, both
deployed, or the peering does not exist. And note the pairing discipline the rest of this repository
already keeps — the binary and a box's bind-mounted TOML are a matched pair, so adding `socks_proxy`
to a counterparty's config is a deploy on their box, not a favour they can do you at runtime.

## Rollback

One config edit, on whichever side is failing.

**On the onion node**, point `[node]`'s endpoints back at a clearnet host (or, if there is none,
remove them and set `peer_expose = "neither"` — the node then dials out, is never dialed, and is
exactly the NAT'd operator it was before this runbook). Restart.

**On the dialing node**, point the peer's `endpoint` and the `[[pay_channels]]` row's
`client_edge_url` back at a clearnet URL, or drop the peering rows. `socks_proxy` may be left in
place: with no hidden-service endpoint in the file it selects nothing, and a proxy that nothing dials
through costs nothing.

Neither rollback touches the daemon, the volume or the address. Leaving the sidecar running with its
`HiddenServiceDir` intact is what makes the next attempt cheap — the address is the expensive thing
to lose, and it is the one thing here nothing can regenerate for you.
