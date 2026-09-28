# A connector's URL resolves to its self-description

**Status:** Accepted — **built** (#1080). `GET /ilp` serves the document, `[announce]` is now `[node]`, and the x402 greeting is a projection of the same source. Completes [0022](0022-a-connector-answers-it-does-not-announce.md) by giving "answering" a single surface, and is what [0046](0046-the-kind-10032-announce-is-removed-a-connector-needs-no-relay.md) left behind when the announce was removed (#1074). Narrows [0003](0003-clean-room-peer-wire-versioned-client-edge.md)'s version-discovery mechanism onto this document. **[0058](0058-a-peering-is-established-from-a-url.md) builds on it** — a peering established from a URL reads this document and nothing else. Extended by [0067](0067-a-route-declares-its-request-shape-and-the-connector-never-reads-it.md), which adds one more fact to what both surfaces publish: what a route wants sent to it. **Amended by [0075](0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md)** (accepted 2026-09-27): the self-description drops `settlements` and publishes each chain's voucher signer. See the Update at the foot.

**Scope:** protocol law — binds every implementation, not just this one. See the [ADR index](README.md).

The falsifier this record carried while it was unbuilt — no file under
`crates/connector-client-edge/src/` registering a `GET` on `/ilp` — is **satisfied and removed**.
`router_with_node_facts` registers `.route("/ilp", post(handle_ilp).get(self_description))`, which
is precisely the line the falsifier said no implementation could avoid.

**`GET` on a connector's own URL returns its self-description**: the facts a stranger needs to
transact with it, as one document, with no ILP packet, no encoder and no protocol knowledge required.
The x402 greeting becomes a projection of it. It carries **facts only** and never accepts a `POST`.

## Why one document, and why at that URL

Before this record a node described itself in **two** places with **different field sets** — the x402
greeting's `extra` block and a kind:10032 announce — and neither was authoritative. Neither was a
superset of the other. That is not a tidiness problem: `requiredTransport` was _enforced_ long before
it was _advertised_, toon-client's guard read a key no announce carried, and every relay publish was
refused. Verified live 2026-08-14 — not one announce in the fleet's corpus carried the key in any form.

**The address is `GET /ilp`** — the same URL that already appears in every config, every greeting and
every operator's runbook. `POST /ilp` remains the packet endpoint; `GET` on it was unrouted and
answered `405`. A stranger who has this node's URL at all therefore has its description, with nothing
further to discover.

The alternatives both fail on a fact. A separate sub-resource (`/ilp/info`) is a second address that
must itself be discovered — reintroducing the step this document exists to remove. A well-known URI
(`/.well-known/toon`) assumes the connector owns the origin root; it does not, and `/ilp` is the mount
point precisely because the root is not the connector's to claim.

## What it carries

**Connector facts only** — things this node either proved at startup or was configured with, about
itself:

- its ILP address(es);
- its public client-edge endpoints, HTTP and BTP, and which carriages it exposes;
- its edge identity — the key a packet is sealed to
  ([0018](0018-a-payload-is-sealed-to-the-terminating-connector.md)). This closes issue #1026: a
  forwarded route is unreachable today because the terminating connector's identity is required to
  seal a packet and is published nowhere;
- per chain, the channel-opening facts a buyer needs — chain id, settlement address, token network
  and its registry, token address, decimals;
- its route prices, and their descriptions once [0044](0044-a-probe-answers-what-a-route-costs-and-what-it-does.md) is built;
- the client transport its routes require, when they agree on one;
- the client-edge versions it serves, and which one unversioned `POST /ilp` resolves to
  (issue #1054 — `GET /ilp/versions` is retired before it was ever built, and
  `client-edge-spec.md` §3.2 is rewritten to point here).

**It does not describe what runs behind the connector.** `relay_url` — an operator's assertion that a
Nostr relay for free reads sits behind this node — is **dropped**. [0046](0046-the-kind-10032-announce-is-removed-a-connector-needs-no-relay.md)
established that a conforming connector must work with no relay in the world, and this was the last
place that assumption survived. A connector is a paid reverse proxy; what is behind it is the app's
business and the operator advertises it elsewhere. The document's meaning stays uniform: **everything
in it is true of this connector**.

**It does not carry per-peer facts.** Caps and peerings are operator-private
([0049](0049-the-cap-bounds-one-packet-is-discovered-by-t04-and-is-set-from-outside.md),
[0006](0006-the-connector-is-mechanism-not-policy.md)): publishing them would disclose who this node
peers with and how far it trusts each.

## Facts only. There is no `POST`.

**This endpoint never accepts a write, and never becomes a surface on which a stranger requests
peering.** It publishes what an operator needs to configure a peering out of band; a peering itself is
created by an operator in the config file or through the operator surface, and by nothing else
([0043](0043-purchasable-peering-is-removed.md), [0006](0006-the-connector-is-mechanism-not-policy.md)).

This prohibition is stated rather than implied because the failure mode is obvious in hindsight and
slow to arrive: a self-description endpoint grows a `POST`, and purchasable peering is back through a
side door years after 0043 removed it.

## Consequences

**`[announce]` becomes `[node]`.** Its surviving fields — `addresses`, `http_endpoint`,
`btp_endpoint` — are exactly the facts a node cannot introspect about itself (a container sees
`0.0.0.0:4000`, never its public URL), and they already feed the x402 greeting's bootstrap identity
(issue #807). They move to a section named for what they are rather than for a verb that no longer
exists. Everything else in `[announce]` dies with the announce (issue #1074).

**Issue #981 closes by construction.** `[announce].solana_chain_id` defaults to `solana:devnet` and is
never checked against `[settlement.solana]`, so a mainnet node announces itself as devnet. It is a
**second declaration** of a fact the settlement backend already holds and verified against a chain at
startup. One authoritative document derives it from that backend, and the class of bug disappears
rather than acquiring a consistency check.

**The greeting is a projection, not a peer.** It keeps its own job — terms for one specific priced
route, in band, to a client that just tried to use it — and stops doubling as a node description.
Where the two disagree, the document is authoritative.

**No caching contract, and no TTL.** The document is generated from live configuration and read when a
client wants it. `ttl_secs` existed because a _pushed_ copy needed a shelf life; a pulled one does not,
and it does not come back.

**Free and unauthenticated**, which is what [0022](0022-a-connector-answers-it-does-not-announce.md)
already means by answering: it decides nothing and reaches nobody who did not ask. Rate limiting uses
the shaper that already guards unresolvable lookups rather than a mechanism of its own.

## Update (issue #1080)

Built, together with [0046](0046-the-kind-10032-announce-is-removed-a-connector-needs-no-relay.md)'s
removal (#1074) — the two could not be separated, because renaming `[announce]` while
`connector announce` still read eleven of its keys would have left a broken subcommand.

What landed, and where the decision above turned into a mechanism:

- **`GET /ilp`** is `connector_client_edge::self_description`, registered as
  `.route("/ilp", post(handle_ilp).get(self_description))`. `POST /ilp` is byte-for-byte unchanged.
- **One source, two projections.** `connector_domain::NodeFacts` holds this node's own facts;
  `NodeSelfDescription::describe` projects the document out of it and `x402::terms_body` projects
  the greeting's `extra` block out of the same value. Nothing assembles either set a second time, so
  the disagreement this record exists to prevent has nowhere to occur.
- **The legacy one-chain greeting object is derived**, not carried. `extra.settlement` is
  `NodeFacts::evm_settlement()` — the per-chain list's own EVM entry — where it used to be a second
  field composed beside the list at startup.
- **`[node]`** carries `addresses`, `http_endpoint`, `btp_endpoint` and nothing else. Every
  announce-only key is parsed solely to be refused **by name**, and so is a stale `[announce]`
  heading, which is refused with the new name in the message.
- **Rate limiting** is the existing `UnresolvableLookupBudget`, shared with channel resolution rather
  than duplicated. The consequence is stated in the handler's own doc: a flood of description
  requests shapes chain lookups and vice versa. That is the price of one bucket, and the shaper waits
  rather than dropping, so it costs latency and never availability.

**Issue #981 is closed by construction, and this is what that means concretely.** There is no
`solana_chain_id` anywhere in the tree — not defaulted, not overridable, not compared against
anything. A Solana entry's `chain` is whatever `SolanaSettlementBackend::connect` reported after
proving the program is reachable, executable and behaves like the deployed payment-channel program.
No consistency check was added, because there is no second source to check against.

**Issue #1026 is not closed by this record alone, and the sentence above claiming it is should be
read narrowly.** What #1026 needs is a path from "I want to pay a forwarded route" to "here is the
key to seal to". This record supplies the **publication**: the terminating connector now publishes
its edge identity in its own self-description, free and unauthenticated. It does not supply the
**discovery**: a client still has to learn the terminating connector's URL, and ND-14 forbids the
first hop from handing over another node's identity. That step is
[0054](0054-an-unsealed-termination-reject-answers-where-to-ask.md)'s unsealed reject (#1083), which
is not built. Until it is, a forwarded route is reachable only when the client already knows the
terminating node's URL out of band. (`GET /ilp/identity` already published the same key before this
change, on each node's own URL — so the gap #1026 describes was always the discovery half, not the
publication half.)

## Amendment (issue #1220): which endpoints a peerable node may omit is `peer_expose`'s call

`[node]` required both `http_endpoint` and `btp_endpoint` unconditionally, with no way to say
"this node opens only one listener". The README's own minimal config (step 1: `client_edge_addr`,
`state_dir`, `[signer]`, `[[routes]]`) never mentions `[node]`, `peer_expose`
([0027](0027-connectors-peer-over-btp-or-http-and-the-raw-tcp-peer-wire-is-deleted.md)) or
`peer_allow_plaintext_endpoints` at all — a reader who followed it to the letter got a node that
serves, but whose self-description carries no endpoints and `"peerCarriages": []`, so a
counterparty's `POST /peers` at it answers `502`. An operator who then wanted to publish just the
one endpoint they actually serve — the common HTTP-only shape behind a single TLS-terminating
reverse proxy — had no honest way to do it: `btp_endpoint` was required even when no BTP listener
was ever opened, so the only way past the loader was inventing a `ws://` line for a listener that
does not exist and publishing that dead URL to whoever asks. That is exactly the lie this record's
no-default rule exists to prevent, arriving by a second door the rule did not cover.

The fix lets an operator omit what they do not serve, and names what a peerable node cannot omit.
Two rules, both enforced **by name** at load, each naming the missing key and the `peer_expose`
value that made it required:

- `btp_endpoint` is required exactly when `peer_expose` opens a BTP peer listener (`"btp"` or
  `"both"`). An HTTP-only node leaves it out and publishes no `btpEndpoint` — not `null`, absent.
- `http_endpoint` is required whenever **any** carriage is exposed, BTP included. A peer covering a
  forward to this node asks its client edge for claim-state over plain HTTP whichever carriage the
  packet itself rides (`POST /ilp/claim-state`, [0058](0058-a-peering-is-established-from-a-url.md)'s
  #1217 correction), so a peerable node with no `http_endpoint` is a node nobody can pay.

A **declared** endpoint is never refused for being "unexposed". Both client-edge listeners are
served whatever `peer_expose` says — `peer_expose` governs only the peer role on them — so an
operator publishing where clients reach this node over a carriage is publishing a URL that answers.
The devnet fixtures are exactly that shape (both endpoints, no peer carriage exposed) and keep
loading unchanged; the symmetric rule first proposed here would have refused every deployed config
until each node repo added a `peer_expose` line. With `peer_expose = "neither"` (the default) both
endpoints may be omitted, and `[node]` may still exist for its `addresses` alone — a legitimate, if
unpeerable, self-description. `docs/protocol/configuration-spec.md` CF-08 is amended to state the
rule and cross-reference CF-17, which is what defines "carriage" and "exposed" here. The README
gains a "Being peerable" step showing the three keys together.

## Update (ADR 0075, issue #1371) — accepted 2026-09-27, implementation pending

[0075](0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md) decision 10 amends what
the document publishes. `settlements` (the TOON channel terms) is dropped and `batchSettlements`
stays. The document also publishes, per chain, the node's **voucher signer** — its settlement
address on EVM, its settlement key on Solana — which is what a peer binds this node's inbound
channel by. The greeting's `accepts[]` is `batch-settlement` entries only.

**Built in part by #1378:** the document publishes `voucherSigners`, one per chain this node pays
x402 on. **Built by #1384:** the document no longer publishes `settlements`; `batchSettlements`
and `voucherSigners` are its settlement facts. The greeting's `accepts[]` is `batch-settlement`
entries only, and TOON's own terms (the quoted amount, the price schedule, this node's addresses)
ride in x402's `extensions.toon` (0075's `## Update (issue #1384)`).
