# An onion address is a host, not a carriage

**Status:** Accepted — built (#1273), **amended in place by issue #1284** (the hidden-service TLD
is a spelling: `.anyone` is a host on the same terms `.onion` is — see the Amendment below).
Extends
[0027](0027-connectors-peer-over-btp-or-http-and-the-raw-tcp-peer-wire-is-deleted.md) by naming what
an onion endpoint is _not_: it adds no third carriage and reopens nothing 0027 closed. Narrows
[0004](0004-value-moves-on-fulfilment.md)'s wire-authentication requirement in exactly one place, by
satisfying it differently rather than waiving it.

**Scope:** connector architecture — internal to this codebase. One clause is protocol-visible: a
`.onion` endpoint published in a self-description ([0050](0050-a-connectors-url-resolves-to-its-self-description.md))
carries a plaintext scheme, and any implementation reading such a document must accept that or be
unable to dial the node.

**Falsifier:** `crates/**/*.rs` matching `PeerCarriage::Onion` — this record claims an onion endpoint adds no third peer carriage; a match outside a comment means one was added anyway, and decisions 1 and 2 are describing a tree that no longer exists.

A `.onion` address is a **host**. It is not a scheme, not a carriage and not a transport. Both
carriages ADR 0027 settled on ride an onion endpoint unchanged: `http://…onion/ilp` is still
ILP-over-HTTP and `ws://…onion/btp` is still BTP. What this repository gains is an outbound SOCKS5
dial path selected by host, an inbound endpoint an operator writes down, and one narrow exemption
from the TLS-only endpoint rule. `PeerCarriage`, `peer_expose`, a route's `transport` policy and the
greeting's `requiredTransport` are all untouched.

## Context

This connector exposes two peer carriages and selects between them by the **scheme** of a peer's
configured `endpoint` (`peer-carriage-spec.md` §2.1): `wss://` is BTP, `https://` is ILP-over-HTTP.
`PeerCarriage::from_scheme` says so, and its doc comment says the set is closed — _"there is no third
value and nothing to add one for."_

An operator wanting to run a node without a public IP, a DNS name or a TLS certificate has one
option today: `peer_expose = "neither"`, which means dialing out and never being dialed. That
operator is real — it is the shape `[node]`'s own documentation calls "the NAT'd operator" — and the
capability they lack is not privacy, it is **inbound reachability**.

An onion-routing network supplies exactly that. Anyone Protocol (a Tor fork; binary `anon`, config
`anonrc`, SOCKS on 9050) publishes a v3 onion address for a local port and routes circuits to it.
The temptation is to model this as a third transport beside BTP and HTTP, and the request that
prompted this record was phrased that way. It is the wrong shape twice over:

- **A third carriage buys one transport, not both.** `PeerCarriage` is a closed two-valued set
  threaded through `peer_expose`, the route-level `transport` policy, `agreed_required_transport`,
  the x402 greeting's `requiredTransport` and `peer-carriage-spec.md` §11's normative spellings.
  Adding `Onion` to it forces a third value into every one of those and still leaves the question of
  _which_ protocol an onion peering speaks unanswered.
- **"Transport" is already taken.** A route carries `transport = "btp"`, meaning the _client_
  transport that route accepts. "Onion transport" would collide with a live config key on the day it
  was written.

Below the transport port there is already one pipeline — a PREPARE that arrived over HTTP is
indistinguishable from one that arrived over BTP — and an onion endpoint changes nothing about that.
It changes where bytes are _addressed_, which is a property of the URL's host.

### Why the TLS rule has to move, and why that is not a weakening

`PeerCarriage::from_scheme` is TLS-only on purpose: a peering carries signed balance proofs (ADR
0004), so the wire must be authenticated. `ws://` and `http://` select nothing unless a node sets
`peer_allow_plaintext_endpoints`, whose own doc comment says it exists for a laptop harness and _"is
not a deployment shape."_

Web PKI certificates for v3 `.onion` names are not practically obtainable, so an onion peering will
speak `http://` or `ws://`. Reusing the global flag to permit that would open plaintext to **every**
peer in order to get it for one, which is precisely the deployment shape that comment refuses.

The narrow rule is available because of what a v3 onion address _is_. The address is a base32
encoding of an ed25519 public key, and the circuit is encrypted and authenticated to that key. A
client that reached `abc…xyz.onion` reached the holder of that key or reached nothing. That is a
**stronger** identity binding than a CA-issued certificate for a DNS name: no third party attests
it, and there is no issuer to mis-issue. ADR 0004's requirement is therefore _satisfied by a
different mechanism_, not waived — which is why the exemption is keyed on the host having a `.onion`
suffix and on nothing else.

### What this does not hide

Stating this in the record rather than in a footnote, because the name "hidden service" invites the
opposite reading and the settlement layer contradicts it:

- Every claim names an on-chain channel and address. Every operator write is RFC 9421-signed under a
  keyid. **Who paid whom is on a public chain either way.**
- The settlement RPC and the `handler_url` are not proxied by this decision. A node reaching a
  public RPC provider does so from its real address, and that provider also sees the transactions
  that node submits. An observer positioned there can link the operator's network location to their
  on-chain identity, and running the ILP wire over onion does not prevent it.

An onion endpoint hides **where a node is reachable**. It does not hide who the operator is, and
this record makes no anonymity claim beyond that sentence.

## Decision

1. **An onion address is a host under the existing two carriages.** No `PeerCarriage` variant is
   added, `peer_expose` is unchanged, and a route's `transport` policy and the greeting's
   `requiredTransport` keep their two values. `peer-carriage-spec.md` §11's spellings do not change.

2. **A `.onion` host permits the plaintext schemes.** `ws://` selects BTP and `http://` selects
   ILP-over-HTTP when — and only when — the endpoint's host ends in `.onion`. This is independent of
   `peer_allow_plaintext_endpoints`, which keeps its existing meaning and its existing scope.
   Because §2.1 must have exactly one implementation, the rule lives beside
   `from_scheme_allowing_plaintext` and is threaded to the runtime-peering path (ADR 0058) rather
   than copied into it.

3. **Outbound dialing selects the proxy by host.** A node configures one SOCKS5 proxy. An endpoint
   whose host ends in `.onion` is dialed through it; every other endpoint is dialed direct. The URL
   must be `socks5h://` — the `h` is not a preference, since no local resolver can resolve a
   `.onion` name and resolution has to happen at the proxy.

4. **The proxy covers the ILP wire only.** Settlement RPC and `handler_url` dial direct. Routing
   settlement through a circuit is a separate decision with its own evidence to gather — circuit
   latency interacts with confirmation semantics and nonce handling on both backends — and is not
   taken here. The consequence is the linkage named above, and it is a limitation of this record
   rather than an oversight.

5. **`connector send` takes a `--socks-proxy <url>` flag.** That verb parses flags and loads no
   config file, so there is nowhere for a config section to reach it. It applies the same
   host-selected rule to both URLs it dials — `--operator` and `--seal-to`.

6. **No new `[node]` key.** A `.onion` URL is a legal value for the existing `http_endpoint` and
   `btp_endpoint`. An onion-only node has no clearnet endpoint to lose, and issue #1220's rule —
   an endpoint is required exactly when `peer_expose` opens that listener — already holds
   unmodified. A second key per carriage is added only when a genuinely dual-homed node exists to
   justify it, and not before.

7. **The operator writes the address down.** The `anon` daemon generates the onion address into its
   `HiddenServiceDir/hostname`; the operator copies it into `[node]`. This is ADR 0050's shape
   exactly — a fact no process can introspect about itself — and the connector neither reads that
   file nor speaks the daemon's control protocol.

8. **`anon` runs as a sidecar.** It is a C daemon with its own terms-acceptance flag, and the
   connector is one Rust binary. The relationship is the one this repository already has with
   `anvil` and `solana-test-validator`: infrastructure the node talks to, never something it
   contains.

## Considered options

**A third `PeerCarriage::Onion`.** Rejected in Context: it forces a third value through five call
sites, and yields one onion transport where the requirement was both.

**Reuse `peer_allow_plaintext_endpoints` for onion peerings.** Rejected: it opens plaintext to every
peer to permit it for one, and that flag is documented as a test affordance rather than a deployment
shape. Widening it would make that comment false.

**Require TLS on onion endpoints anyway, self-signed with pinning.** Rejected: it adds a certificate
lifecycle and a pin-distribution problem to obtain an identity binding the onion address already
provides, more weakly than the address provides it.

**Proxy every outbound dial once a proxy is configured.** Rejected: it breaks the mixed case — one
onion peer and one clearnet peer — with no way back, and it silently reroutes settlement, which
decision 4 declines to do on purpose. Host-selection needs no configuration surface at all, because
the address already carries the answer.

**Per-peer `proxy =` on `[[peers]]`.** Rejected: it restates in a second key what the endpoint's
host already says, and two places to say one thing is how a peering ends up dialed the wrong way.

**An env var or a default port for `connector send`.** Rejected: this repository has no
environment-variable layer (ADR 0009), and a value that changes where a signed operator write is
sent should not arrive invisibly. A hardcoded `127.0.0.1:9050` fallback assumes a deployment.

**Read the onion address from `hostname`, or query the control port.** Rejected: both couple the
connector to a daemon's internals for a value that never changes once generated, and both put a
node's own self-description at the mercy of a sidecar's filesystem layout.

## Consequences

**An onion node is invisible to the clearnet fleet.** Its `GET /ilp` publishes a `.onion` URL, and a
peer without a proxy cannot dial it. This is not a new class of condition — `CONTEXT.md` already
holds that _"reachability is the only registry"_ — but it is a stronger statement than "unreachable
today": such a node cannot be reached by the existing devnet boxes at all.

**The hidden-service key is now operational state.** If `HiddenServiceDir` is not on a persisted
volume, the node's address changes on every restart and every counterparty's config goes stale
silently. This belongs in the operator runbook beside `state_dir`, and for the same reason.

**The IP-to-on-chain-identity linkage stands.** Decision 4 leaves it, and this record names it so
that a later decision to proxy settlement is taken on evidence rather than discovered as a gap.

**CI gains a SOCKS5 dial test, not an onion one.** The connector's own change is "dial through
SOCKS5" and "accept a `.onion` host as a valid endpoint", both testable against a local SOCKS5
server with no network and no daemon. A real-`anon` topology under `local/` proves the composition
end to end and is deliberately **not** on the CI gate: this repository's rule is that a test either
runs or fails loudly (`require_anvil()` panics under `CI` rather than skipping), and a gate that goes
red when a third-party anonymity network has a bad day is that rule inverted, not honoured.

That topology cannot assert on `--expect-fulfill` alone, for the same reason `two-hop` cannot: a
fulfilled packet proves nothing about the circuit, and a SOCKS dial that silently fell back to a
direct connection would go green. It puts each connector on its own network with no route between
them, so that a direct dial is **structurally impossible** rather than merely unobserved, and it
keeps `two-hop`'s claim-journal read so the peering is shown to have charged and not merely
connected.

## Amendment (issue #1284): the TLD is a spelling, and both spellings are hosts

Anyone Protocol's `anon` **renamed the hidden-service TLD** between v0.4.9.7 and v0.4.10.2, and the
rename is total in both directions. v0.4.10.2 writes `<56-base32>.anyone` into
`HiddenServiceDir/hostname`, routes that name through its SOCKS port, and refuses the same address
spelled `.onion`; `strings anon | grep -cF .onion` on it returns **0**. v0.4.9.7 does the exact
opposite. Neither release resolves the other's spelling, so the two are not two names for one
network reachability — they are two networks as far as any single daemon is concerned.

This record was written against the older release, and `toon-client` pins the newer one. The result
was an interoperability break in which neither side was wrong on its own: a node brought up by
`local/onion` published an address that client refused, and the client dialed a TLD that daemon
could not resolve.

**Both suffixes are accepted.** `is_onion_endpoint` — still the one implementation of the host rule,
per decision 2 — matches a host ending in `.onion` **or** `.anyone`. Nothing else about this record
changes: not the decisions, not `PeerCarriage`'s two values, not `peer_allow_plaintext_endpoints`'s
meaning or scope, and not decision 4's refusal to proxy settlement.

**Why the argument carries over unchanged.** Decision 2's exemption is earned by what a v3 address
_is_ — the base32 encoding of the ed25519 public key the circuit is encrypted and authenticated to.
That is a property of the address, not of the label after the last dot. A client that reached
`abc…xyz.anyone` reached the holder of that key or reached nothing, exactly as at `.onion`. ADR
0004's requirement is satisfied by the same different mechanism.

**Why `.onion` stays.** Dropping it would follow upstream and buy nothing this repository wants. The
check is a suffix test either way, so keeping both costs one `ends_with`; a node whose operator runs
the older daemon keeps working; and Tor's own `.onion` names have the identical property the
exemption is granted for. What would be gained by narrowing is a config that fails to load for a
reason that is upstream's release cadence rather than this connector's rule.

**The narrowness that matters is untouched.** These are suffixes and not substrings, so
`anyone.example` and `notreally.onion.example` are ordinary clearnet hosts and a plaintext scheme at
either is still `PeerEndpointScheme`. The suite that says so runs every host-suffix case at both
spellings (`crates/connector-runtime/tests/an_onion_host_is_a_host.rs`).

**The vocabulary keeps the word "onion".** `CONTEXT.md`'s **onion endpoint**, this record's title and
`is_onion_endpoint` all name the mechanism — an address that is a key, reached over a circuit through
a SOCKS5 proxy selected by host — and not the TLD it is spelled in. Renaming them to track a
third-party daemon's release notes would churn a thousand citations to say the same thing. What the
name must never mean again is _one_ spelling: that reading is what this amendment closes.

**`local/onion` runs the current daemon.** ghcr publishes no image for v0.4.10.2 — its
`ator-protocol` tags stop at v0.4.9.7 — so `local/anon-image` builds one by overlaying the official
release binary, sha256-verified, onto that image, and both hidden-service topologies use it. The
committed placeholder hosts are spelled `.anyone` to match what that daemon actually writes, and
`local_topologies_load.rs` holds the placeholder's TLD and the image's pinned version to one fact.
