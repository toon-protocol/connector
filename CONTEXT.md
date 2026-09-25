# Connector

The bounded context of a node that forwards packets for payment. Terms only — decisions
live in `docs/adr/`.

**The connector terminates payments the way nginx terminates SSL.** Value arrives wrapped in a
protocol the app never speaks; at the last hop the connector unwraps it, verifies it, and hands the
app ordinary HTTP that was already paid for. Read the rest of this glossary through that sentence.
A **route termination** is where the unwrapping happens, the **app** is the origin server behind
it, and the connector in front is a paid reverse proxy — not a library the app imports, and not a
role of its own beside the two.

## Language

### Forwarding

**Connector**:
A node that accepts a packet, decides where it goes, exchanges value for carrying it, and
hands it on. Never interprets the payload of a packet it forwards — opacity is a property of
carriage, not of the node. At a route termination the same node does interpret it, because
that is what terminating means.
_Avoid_: terminator, connector-as-terminator, gateway

**App**:
The payment-oblivious service a connector delivers to at the end of a route. It settles nothing,
holds no channel and is never told which destination was addressed. It IS told who paid, how much
and on what chain — `X-TOON-Payer`/`X-TOON-Amount`/`X-TOON-Chain`, from the client claim the
delivering connector verified itself (ADR 0040) — and told none of the three when that connector
was not the one paid. Either way, whatever arrives at one of its handlers was paid for, under that
handler's one price (ADR 0020) — `X-TOON-Amount` states what this packet was actually charged,
which for a route priced by size is not a figure the app's own route table could have known
(ADR 0065).
_Avoid_: BLS, Business Logic Server, agent runtime, backend

**Handler**:
The app's receiving endpoint, and the unit a price attaches to: one handler, one price. An app
charges differently for different work by exposing a handler for each. One handler needs no second
handler to charge differently by _size_ — that is the price's own slope (ADR 0065).

**Description**:
Operator-written text saying what the work behind a route is. Attaches exactly where a price
attaches — one handler, one description — comes from the connector's own configuration and from
nowhere else, and rides both the greeting and a probe's reject. A menu, not a warranty: whoever
reads one is reading text from a stranger. _(Decided and not yet built — ADR 0044.)_

**Request**:
The operator-declared table naming what a client must send to use a route — protocol, parameters,
whatever the app behind it expects. Written as `[[routes]] request = { ... }`, converted to JSON at
load and published verbatim on that route's self-description entry and on the greeting for that
destination; the connector confirms only that it **is** a table and reads none of its keys. Sourced
by the operator writing it down, never by asking the app — matching the declaration against what the
app actually registered is the app's own repository's problem, not this one's (ADR 0067). Absent,
not empty, on a route that configured none.
_Distinct from_: a route's **description** above — free text about what the work **is**; `request`
is structured and says what to **send**, and building it does not build ADR 0044.

**Packet**:
The unit of forwarding: a destination, an amount, an expiry, and a payload that is opaque to
every hop that carries it. Every packet terminates in either fulfilment or rejection. Its
**semantics are ILPv4's and its encoding is this project's own**, and the two are not
byte-compatible — deliberately, and ratified rather than tolerated
([ADR 0063](docs/adr/0063-the-ilp-packet-is-toons-dialect-not-rfc-0027s.md)). Where the
distinction matters, say which of the two you mean.
_Avoid_: speaks ILPv4 (retired — it names the semantics and implies the bytes; the accurate form
is "ILPv4 semantics, TOON encoding")

**Condition**:
Retired from the wire (issue #1269, ADR 0069). A packet no longer carries one: it was a commitment
minted by the sender, invariant across every hop and distinctive per packet — exactly the shape of
a cross-hop join key, and it paid for nothing a hop was actually charged to check. What survives is
the sender's own end-to-end check, over the fulfilment directly.
_Avoid_: execution condition (as a thing a current packet carries), hashlock

**Fulfilment**:
The proof that a packet was delivered to its intended receiver. It proves delivery; it does not
move value — a packet carries its own claim. At a route termination the terminating connector
derives it from the request's own sealed secret (ADR 0019); no hop upstream checks it any more
(ADR 0069) — only the sender does, against that same derivation.
_Avoid_: receipt, proof of payment, preimage (when the layer is already clear)

**ILP address**:
The name a connector answers to, and the thing a route's prefix matches against. **Self-asserted: a
claim, not a grant.** Nothing allocates one, no registry records one, and no connector is given one
by another — an operator writes down the address their node claims. An address means nothing until
somebody else routes to it, so **reachability is the only registry**, and two nodes claiming the same
address is resolved by whoever declines to carry for one of them. Choosing a name beneath a peer's
address is a courtesy that keeps their table small, never a delegation that binds anyone.
_Avoid_: allocated address, assigned address, address space (as though it were owned)

**Route**:
A mapping from a destination prefix to the next hop that should carry it.

**Path**:
The sequence of hops a packet actually takes from the sender to the route that terminates it —
each hop a **peering** somebody chose, taking its fee and carrying the packet on. A destination is a
prefix; a path is what a sender commits value to, because every hop holds the claim it was handed
and any hop may decline to carry. The exposure on a path is therefore the one packet in flight, and
it is bounded by sizing packets to the path's record — small on a new path, larger on one that has
fulfilled — never by escrow. Two paths to the same prefix are two different things to trust.
_Avoid_: destination (when the path is meant), route (a route is one hop's mapping, not the whole
path)

**Static route**:
A route given to the connector by its configuration. Durable across restarts, and always
beats a leased route for the same prefix.

**Leased route**:
A route pushed in by a controller with a time limit, which lapses unless renewed. Expiry is
the mechanism by which a route to an unreachable peer stops being used.
_Avoid_: learned route, dynamic route

**Runtime route**:
A route or peer written through the operator surface that survives a restart — the third shape
beside static and leased. Durable like a static route, mutable like a leased one, and never able to
take a key the config file owns: a colliding write is refused outright rather than shadowing or
being shadowed.

**Controller**:
Whatever decides the connector's leased routes and peering. Outside the connector by
definition — the connector never learns, announces, or discovers. The line is about **deciding**,
not about fetching: a connector told to reach a counterparty will read that counterparty's
self-description to learn how, exactly as it dials a handler's URL. What it never does is choose
whom to peer with, or find one it was not pointed at.

**Operator**:
The human or organisation that runs a connector. Owns the config file, the identity key and the
operator surface, and is the only party that creates a peering or publishes a route. Distinct from
a **controller**, which is automation an operator points at a connector; and distinct from the
connector itself, which decides nothing the operator did not write down. Some verbs belong to the
operator and not to the process: announcing is one.

**Announcing**:
Pushing facts about yourself into a network unprompted. A connector never does this: deciding to
participate in a discovery network is the controller's business.
_Distinct from_: answering, which a connector does do.

**Answering**:
Telling whoever asks what your own configuration already says — your identity, and what a route of
yours costs. Mechanism, not policy: it decides nothing, and reaches nobody who did not ask. A
sender asks directly and pays through the network, so what it learns by asking is not something an
intermediary can substitute.
_Avoid_: discovery (for this — a connector answers, it does not discover)

**Self-description**:
The one document a connector answers a `GET` on its own URL with: everything a stranger needs to
transact with it, and everything in it true **of that connector** — a fact it either proved against a
chain at startup or was configured with. Its addresses and public endpoints, the key a packet is
sealed to, per chain what opening a channel takes, what its routes cost. Free, unauthenticated,
generated from live configuration when asked, and never a place a write is accepted. It carries
nothing about the software behind the connector, and nothing about who it peers with. Because it
holds every public fact about a counterparty, it is the whole of what one operator must give another
to be peered with.
_Distinct from_: a **greeting**, which is terms for one priced route, in band, to a client that just
tried to use it — and which is a projection of this, so the two cannot disagree.
_Avoid_: manifest, announce, discovery document, node info

**Greeting**:
What a connector answers an unpaid request to a priced route with: that route's terms — what it
costs and what is needed to pay it — instead of the work. This is what makes a connector that sells
safe to be reachable at all, because the unpaid case gets a defined, useful, unpaid answer rather
than free service. Its node facts are a projection of the **self-description**: one source, so a
route's enforced behaviour can never run ahead of what is published about it.
_Avoid_: 402, x402 (those name the carrier, not the thing)

**Route termination**:
The property of a route that ends at this connector, where a packet becomes a delivery to
an app. The point at which a payload stops being opaque: the terminating connector reads the
packet's envelope to know what request to make.

**Client destination**:
An address that resolves to a live client session rather than to a handler. The connector delivers
to that client and never terminates: it does not open the payload, does not derive a fulfilment,
and takes back the one the client produced or rejects the packet. A destination is never both this
and a route termination — an overlap is a reported configuration error, not a precedence question.

**Envelope**:
What a packet carries: going in, the request a terminating connector is to make of an app — a
method, a target, headers and a body; coming back, that app's response — a status, headers and a
body. One shape, two directions. It is a description of an HTTP message, not an HTTP message:
the app is handed ordinary HTTP, but nothing on the wire is text to be parsed.
_Avoid_: inner request, proxied request, HTTP envelope (when the layer is already clear),
response envelope (as a separate term — the response is an envelope)

**Target**:
The path inside an envelope, naming which of an app's endpoints the request is for. Always resolved
_beneath_ the route's own configured handler path, never in place of it — so a packet can address
more of an app than one entry point, and can never reach a neighbouring route to buy its work at
this route's price.

**Gift wrap**:
The sealing of a packet's payload so that only its intended reader can open it. A sender seals to
the terminating connector's identity, and carries in the wrap the secret that packet's fulfilment
derives from; that connector seals its answer back with the same secret. Because no hop between
them can open either, opacity in carriage is a property of the packet rather than a rule every hop
is trusted to keep. A reject raised short of the termination cannot be sealed — no secret is shared
with the sender — which is what makes a sealed reject proof that the destination itself said no.
The converse does not hold: an **unsealed** reject proves nothing about who refused, because a
termination that never recovered the secret — no identity key, or a wrap it could not open — also
answers in plaintext. Sealed identifies the destination; unsealed identifies nobody.
_Avoid_: encryption (when the layer is already clear), wrapper, seal

**Identity key**:
The key that names a connector to everyone outside it. A sender seals to it, so it is what makes a
packet deliverable at all; and because a fulfilment derives from what it opens, it is load-bearing
for payment and not only for confidentiality. Rotating it invalidates the fulfilment a sender's own
end-to-end check expects for anything already sealed to the old one.

**Packet plane**:
The part of a connector on the path of every packet — routing, claim handling, forwarding.
_Avoid_: data plane, hot path

**Operator surface**:
The part of a connector that runs at human frequency — configuration, inspection,
lifecycle. Never on the path of a packet it did not originate. The exception is the whole of
it: `POST /packets` puts a packet on the path, because originating one is an operator act and
not carriage. What makes that safe is authentication rather than payment — an operator does not
pay their own connector, so the credential is a **write key** and never a **covering claim**.
_Avoid_: control plane, admin

**Dashboard**:
The page the operator surface serves at `/dashboard`: the surface's reads on one screen, and a
form for each write that can be made at runtime, signed in the operator's browser. A client of
the surface that the node happens to ship, with no authority of its own — the bearer token and
the operator key it is fed are the operator's, and the key never leaves the browser.
_Avoid_: admin panel, control panel, console

### Protocol surfaces

**Peering**:
The configured bilateral relationship between two connectors: a counterparty key, a carriage to
reach it on, a fee, and a cap. Created by an **operator** — in the config file or through the
operator surface — and by nothing else. It cannot be bought, learned, earned or announced into
existence. An operator establishes one by naming the other node's URL, so its **identity is
trust-on-first-use**: whoever that URL answers as is who the peering is with, vouched for by nothing
beyond the operator's own vetting of the URL. Never describe one as pinned, verified or attested.
The fee and the cap are the operator's policy about that counterparty, and are held once by the
peering rather than repeated on each route through it.

**Peer semantics**:
What a peer interaction _means_ — claim exchange, fees, reject codes,
accumulated cost. Both ends are operator-controlled. Says nothing about where the bytes ride:
that is carriage, below.
_Avoid_: peer wire (it named a deleted transport and this layer at once; see ADR 0027)

**Peer carriage**:
Where a peer interaction's bytes ride. There are two, and a connector may expose both: **BTP** and
**ILP-over-HTTP** — the same two the client edge already serves. Selected by the endpoint's
**scheme**, `wss://` and `https://`, which is also why there is no third: a carriage is a protocol,
and an address is not one (ADR 0070). Which carriage a connector exposes, and which it dials for a
given peer, is operator policy, never a protocol constant. Below the transport port there is one pipeline: a PREPARE that arrived
over HTTP is indistinguishable from one that arrived over BTP, and peer behaviour that exists on
one carriage and not the other is a defect rather than a property of the carriage.
_Avoid_: peer wire, peer transport (when the layer is already clear)

**Onion endpoint**:
A published endpoint whose host is a hidden-service address — `.onion` or `.anyone`, the two TLDs
the `anon` daemon has published — the node reachable over an onion-routing network rather than at a
DNS name and an IP. Not a third **peer carriage** and not a transport: the
carriage is still BTP or ILP-over-HTTP, and an onion endpoint is where that carriage's bytes are
_addressed_. Because a v3 onion address **is** the public key the circuit is authenticated to, such
an endpoint carries its own authentication and needs no TLS — the one place the plaintext schemes
select a carriage, and a narrowing of ADR 0004's requirement by satisfying it rather than waiving it.
Hides where a node is reachable, never who it pays: a claim names an on-chain channel and every
operator write is signed under a keyid. Dialed through the one `socks_proxy` a node configures,
selected by the endpoint's host and by nothing else — no per-peer key, no all-outbound mode
([ADR 0070](docs/adr/0070-an-onion-address-is-a-host-not-a-carriage.md), built, amended by #1284).
The word names the **mechanism**, never one TLD: `anon` renamed the suffix it publishes between
releases, both spellings are accepted, and `is_onion_endpoint` is the one place that is decided.
_Avoid_: hidden service, onion transport, third transport, Anyone transport, `.anyone` endpoint —
and note that **transport** is already taken: a route's `transport` is the _client_ transport it
accepts.

**Settlement circuit**:
The circuit a settlement table's RPC rides when that table sets `rpc_via_socks_proxy = true`: the
node's one `socks_proxy`, authenticated with a SOCKS username fixed per chain
(`toon-settlement-evm`, `toon-settlement-solana`), so each chain stays on its own circuit and off the
ILP wire's. Every client of the table's `rpc_url` rides it (the backend, and on EVM the
channel-index syncer and the rate source), and none ever falls back to a direct dial. It hides the
node's address from the RPC provider; it does not hide the node's keys, its transactions or its
payments, which are on chain either way
([ADR 0073](docs/adr/0073-settlement-rpc-may-ride-the-circuit-once-every-wait-on-it-is-bounded.md)).
_Avoid_: settlement transport, proxied settlement, hidden settlement.

**Interaction**:
The unit a role attaches to: one BTP session, from its websocket upgrade to its close, or one
HTTP request.

**Peer role**:
The authority of one interaction — `peer` or `client`. Decided by a signature, never by which
listener the bytes arrived on and never by a shared secret: an interaction is a `peer` only if it
carries a claim on a channel one peering configures, whose signature verifies against the
counterparty key that peering configures. **The claim names its own peering** — a channel belongs to
at most one — so nothing has to be asserted alongside it and nothing weaker is consulted first.
There is no third role, no unroled state, and no fallthrough — anything that is not a proven peer is
a client.
_Avoid_: peer wire, peer session (for the role rather than the connection)

**Client edge**:
The protocol a client speaks to the connector it attaches to. The far end is installed on
machines the operator does not control.
_Avoid_: client API, ingress

**Vector**:
A committed input/output pair — an encoded packet, a wrapped packet, an envelope, a shared secret,
the fulfilment it derives — that every implementation replays as its own suite. Vectors are generated
from the properties, never captured from whatever an implementation happened to emit, and reproducing
them is what conformance means. Prose describing the wire is not normative; these are.
_Avoid_: fixture, golden file, test case (when the cross-repo contract is what is meant)

**Peer wire** _(retired term, [ADR 0027](docs/adr/0027-connectors-peer-over-btp-or-http-and-the-raw-tcp-peer-wire-is-deleted.md), issue #679)_:
One word for three things: a raw-TCP transport, the semantics layer riding on it, and the
direction an arrival came from. The transport was deleted and never carried a production packet;
the other two are **peer carriage** and **peer role** above. Retired rather than renamed, because
each sense needed a different word. Still appears, deliberately, in three places: ADR bodies and
five ADR filenames, which are historical records and do not move; `peer_wire_addr`, which
`connector-config` parses **solely to reject** an old config that still sets it; and
`STORE_PEER_WIRE_BIND` in `infra/`, the same tombstone convention. Deleting either identifier
would let a stale config load with the key silently ignored.

### Value

**Payment channel**:
A two-party agreement, anchored on a chain, that lets value move between the parties many
times while touching the chain only to open, top up, and close. **Identified by its participants**,
not by a name either party chose: both sides compute the same identifier from the two of them and
the token, so either can ask the chain whether it already exists without being told anything. At
most one is live per pair per token, on every chain — a pair that has settled starts a fresh one
rather than holding several at once.
_Avoid_: channel (when ambiguous with a route or a stream)

**Claim**:
A signed statement of a payment channel's cumulative state, handed from payer to payee.
Each claim supersedes the last, so a lost claim costs nothing and a replayed claim gains
nothing. A claim has a **scheme**: `toon-channel`, or — at the client edge only —
`batch-settlement`, whose claims are **vouchers** (ADR 0074).
_Avoid_: receipt, payment, balance proof; "voucher" for a `toon-channel` claim

**Voucher**:
A claim under x402's `batch-settlement` scheme (ADR 0074).

**Sponsor**:
A connector's Solana settlement key, in the seats it takes on a client's x402 `batch-settlement`
channel: fee payer and `rent_payer` of the `payment-channels` `open`, which it co-signs through the
public sponsor endpoint, and the channel's `payee` (ADR 0074 decisions 5 and 9). Always the
receiving operator itself, never a third party — a third-party sponsor could seal the channel before
this node lands its latest **voucher**. It pays the fees and floats the rent, which comes home at
`reclaim`, and only above a published minimum deposit. It controls when a channel closes, never
where the money goes.
_Avoid_: facilitator (x402's relayer, which on Solana would take these seats itself)

**Covering claim**:
The claim that pays for one particular packet, carried **with** it rather than trailing behind it.
A packet arriving without one is greeted, not carried. This is what removes accumulation from the
model: nothing is ever owed between packets, so there is no window for a counterparty to walk away
inside. **Every PREPARE a connector sends now carries one** (ADR 0042, issue #1145): a peering this
node forwards to must name the channel it pays from (`[[pay_channels]]`, refused at load without
one), and a forward it cannot cover is refused rather than carried. On **arrival** it is enforced at
the client edge and at a priced termination unconditionally; on a _forwarded_ arrival it is enforced
per peering, behind `forwarded_claim_enforcement` (issue #1142), which still defaults to observing.

**Nonce**:
The counter that orders `toon-channel` claims within a channel. A payee accepts such a claim only
if its nonce advances. A voucher has none; its amount orders it.

**Watermark**:
The highest nonce a payee has accepted on a channel — for a voucher, the highest cumulative
amount, which the next voucher must strictly exceed.

**Exposure** _(retired term, [ADR 0033](docs/adr/0033-the-exposure-machinery-is-retired-not-restated.md), issue #882)_:
Value a payee had delivered but did not yet hold a claim for, under the pre-#868 credit window.
One packet under normal flow; more only when a payer had fulfilled packets and stopped claiming.
Retired by ADR 0033: nothing tracks it, and no projection produces it. The reasoning was that a packet
would carry its own claim (ADR 0042) and so leave nothing trailing — **true of every PREPARE this
connector sends since issue #1145**, and, on a forwarded _arrival_, true of a peering an operator has
set `forwarded_claim_enforcement = "enforce"` on. The retirement is still stated on ADR 0033's own
terms rather than on that one's, because it was decided before either was built. Kept here because
the term still appears in historical prose (`docs/protocol/peer-semantics-pre-868.md` §3.2–§3.4, §5.3; [`docs/protocol/money-model-pre-868.md`](docs/protocol/money-model-pre-868.md)).

**Ceiling** _(retired term, ADR 0033, issue #882)_:
The exposure a peering relation tolerated before the connector stopped forwarding for that
peer. Retired along with exposure, above. Not to be confused with the **cap** below, which
bounds one packet rather than an accumulation.
_Avoid_: credit limit, debt limit

**Cap**:
The largest amount a connector will forward to one peer in a single packet. A packet needing
more is refused with `T04`, never carried and never split — and that reject's message states the
cap, which is the only way a sender learns it. Bounds a single packet, not an accumulation — there
is no accumulation, because a packet carries its own claim. The cap is how far a connector trusts a
peer, expressed as the most it is willing to lose in one theft; the number comes from outside the
connector, which never raises its own (ADR 0049).
_Avoid_: ceiling, limit, liquidity bound

**Flush** _(retired term, ADR 0033, issue #882)_:
Sending a claim that would otherwise have waited to travel with the next packet to that peer.
Bounded how long a payee's trailing exposure could persist when traffic stopped; retired along
with exposure, above. Not to be confused with `peer-carriage-spec.md` §6.4's still-live
`Toon-Flush-Requested` hint, which prompts a payer but binds nothing.

**In flight**:
The state of a packet that has been forwarded but has neither fulfilled nor been rejected nor
expired. An in-flight packet carries value — its claim rides with it — so value is at risk
between the forward and its outcome. Bounding that risk is the sender's business, not the
protocol's: small packets, and larger ones only on a path that has earned it.

**Journal**:
The durable record of what was signed or is otherwise irreversible — claims sent, claims accepted,
and the watermarks that came with them. It is the only money state the connector persists, which is
why recovery is replay rather than reconciliation between two stores that can disagree.

**Projection**:
Money state derived by replaying the journal — per-peer balances. Never a source of truth, always
rebuildable. (It once projected **exposure** as well; that is retired, ADR 0033.)

**Settlement**:
Making a claim's promised value real on-chain, by redeeming the latest claim or by
cooperative close. Rare and deliberate — the opposite of claims, which are constant and
automatic.
_Avoid_: payout, redemption (as a synonym for the whole act)

**Fee**:
What a connector charges to carry one packet across one peering relation. Flat per packet,
not proportional to the amount carried — and not varying with where the packet is headed, because
it pays for this hop's work and that work is the same whatever the destination. One number per
peering, held by the peering, denominated in that peering's unit — which at a hop crossing a
denomination is the **outgoing** leg's, subtracted after the conversion and not before
([ADR 0071](docs/adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md)). What varies
by destination is the **price**; what a hop earns for crossing a denomination is the **spread**,
a separate earning.
_Avoid_: commission; using fee for the dealing margin — that is the **spread**

**Price**:
What a terminated route charges for the work the app does. Distinct from a fee — a fee buys
carriage, a price buys the thing at the end. A **schedule** over the packet's payload length
since [ADR 0065](docs/adr/0065-a-price-is-a-schedule-over-payload-length.md): a `base` every
packet pays, plus a `per_kib` for each started kibibyte, and **flat** exactly when that slope is
zero — which is how every route the fleet runs is priced and the only shape that existed before.
What varies with size is the **charge**; the price is the rule that produces it.
The length measured is the sealed payload's, never anything inside it — a property of carriage,
so a connector still prices without ever interpreting what it carries. Pricing
granularity is handler granularity: an operator publishes a route per handler, and charges
differently for different _work_ by pointing at a different handler — charging differently for
the same work at different sizes is the slope's job, not a second handler's.
_Avoid_: per-byte price (the unit is a kibibyte)

**Charge**:
What one packet actually costs at one terminated route: that route's **price** evaluated at that
packet's payload length ([ADR 0065](docs/adr/0065-a-price-is-a-schedule-over-payload-length.md)).
A price is a rule and a charge is its answer for one packet — the two are the same number only
while the price is flat, which is why they are separate words. Every gate that takes money takes
the charge: the client edge's claim gate on both carriages, a peer arrival, a probe's reject, and
the termination itself, all computing it from the same bytes so they cannot disagree.
_Avoid_: using **price** for this — a price is what the route charges, a charge is what this
packet cost.

**Cost**:
What a caller must send for a packet to be delivered: the fees of every hop that carries it, plus
the **charge** of the route that terminates it. A reject states the cost of the path _that
packet_ travelled, which is how a probe discovers it. The sum only — never the per-hop breakdown,
and never the split between fees and price. The sum arrives in the asker's own unit: each
denomination boundary a reject crosses converts the running figure as it passes
([ADR 0071](docs/adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md)), so the number
is always readable where it lands and the wire never needs to name a unit. Because a terminating
charge can depend on payload length (ADR 0065), a probe's figure is exact for a packet its own
size; what answers every size is
the terminating node's published **price**, on its greeting and its self-description.
_Avoid_: total fee, quote

**Rate**:
The declared terms on which one connector crosses one denomination boundary: a rational over
**base units** — so many of the outgoing channel's for so many of the incoming one's — with the
two tokens' decimals folded into the ratio and direction included
([ADR 0071](docs/adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md)). Declared or
absent, and absence is the safety rule: a pair this node declared nothing for is refused with a
final **`F02`** rather than passed through at an implied 1:1, and a rate whose observation has
aged out past its ttl with a retryable **`T00`** rather than dealt on dead. Nothing ever converts
silently. A node that declares no tokens resolves no denomination, crosses no boundary and
forwards unconverted at the flat fee — which is every node this repository ships today; a node
that declares them is held to the rule on every ordered pair it can resolve, same asset or not,
because USDC on two chains is two of them. An operator who wants such a pair carried at par says
so in a row: there is no 1:1 rate to fall back on, deliberately. A rate is one connector's own
posted term, never a network fact: the network learns it the way it learns every cost, by probing
the route.
_Avoid_: exchange rate (a market's number; this is one connector's posted one), conversion factor

**Spread**:
What a connector earns at a denomination boundary: the part of the **mid** — the unwidened rate
the node holds for a pair, sourced or hand-tended — that it keeps when it deals, taken against
the sender in whichever direction the packet runs, and paid for carrying the dealing risk: a
stale source, a shoved pool, a packet held against a moving price
([ADR 0071](docs/adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md)). Declared as a
**fraction** strictly below one — as `max_move` and a static rate are, and for the reason ADR 0010
deleted the basis-point fee: there are no floats on this path, and an operator dealing at half a
basis point writes `1/20000` rather than watching it round to zero. A separate
earning from the **fee**, which buys carriage; a hop that crosses no boundary earns no spread.
_Avoid_: arbitrage (riskless profit between venues — this is priced risk), margin

**Segment**:
A maximal run of hops sharing one denomination. One segment, one unit: every amount, fee and
running cost inside a segment is in that segment's unit, and a **path** is its segments joined at
denomination boundaries
([ADR 0071](docs/adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md)). A
single-segment path is the only kind that existed before 0071, which is why older prose may use
"path" where it means this word.

**Minimum delivery** _(retired term, [ADR 0057](docs/adr/0057-minimum-delivery-is-retired-a-claim-bounds-erosion.md), issue #1143)_:
The amount a packet declared must reach its destination, checked by every hop after its own fee
and answered with `R01` when it could not be met. Retired: once a packet carries the claim that
pays for it (ADR 0042), the covering claim is already banked when a hop evaluates the floor, so
rejecting on it returns the sender nothing and only moves where the packet dies. What bounds
erosion is the claim itself — a hop mints one for the packet's **forwarded** value, so it holds a
claim for at least what it passes on, and that chains. The field, both its carriage bindings and its
two vectors are all deleted. `R01` is **not**: only its floor meaning went, and the code still
answers RFC 0027's own case — a hop's fee alone exceeding the arriving amount, so nothing would be
forwarded (ADR 0057 as corrected, ADR 0051). Kept here because the term still appears in historical prose
(`docs/protocol/peer-semantics-pre-868.md` §4, §5.1;
[`docs/protocol/money-model-pre-868.md`](docs/protocol/money-model-pre-868.md)) and in clauses
marked retired.
_Avoid_: floor, guaranteed delivery, minimum amount

**Probe**:
A packet sent in the expectation that it will be rejected, in order to learn from the reject
what the path costs. Not a distinct kind of packet — only a way of using one.

**Signer**:
What holds a connector's keys and signs with them — claims, settlement transactions, operator
writes. A local key or a key-management backend, with rotation, and nothing more: no mnemonic
recovery, no seed management, no human authentication. Those belong to an end-user wallet, which a
connector is not.
_Avoid_: wallet, key manager, custody

**Settlement backend**:
The chain-specific implementation of opening, funding, closing and redeeming for one
chain.

### Storage

**Store**:
The storage node.
_Avoid_: DVM

### Operations

**Breaking deploy**:
A build that makes a configuration a box already runs invalid — a new required key, a renamed
field, a narrowed type. It may never ride an automatic tag move: either the change is made
backward-compatible first, or a promotion lands the config before the image.

**Promotion**:
Moving the tag a box follows to one specific build, deliberately. The only moment at which a
candidate image and the fleet's committed configuration are in the same place, and therefore the
only place a breaking deploy can be caught.
