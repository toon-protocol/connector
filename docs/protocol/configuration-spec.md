# Connector configuration

**Status:** **Normative for its numbered rules.** Per [ADR 0047](../adr/0047-the-configuration-schema-is-implementation-detail-capabilities-are-law.md),
what binds is **what an operator can express**, not what the file looks like. Per
[ADR 0045](../adr/0045-a-behavioural-rule-is-normative-prose-until-its-vector-lands.md), a behavioural
rule is normative prose until a vector covers it — and configuration is **not vectorable**, so these
rules are prose-normative permanently rather than provisionally. **They do not enter the debt ledger.**

**Consumers:** anyone writing a second connector, and anyone operating this one. §1 is the contract;
§2 is this implementation's file, which binds nobody.

**Vocabulary:** [`CONTEXT.md`](../../CONTEXT.md). MUST, MUST NOT, SHOULD, MAY per RFC 2119.

---

## What this document is, and is not

A second implementation of TOON is **not** required to read this connector's TOML. It is required to
be configurable to _do_ what this connector can be configured to do, and to refuse what it must
refuse.

That distinction is not a hedge. Every fact a counterparty can observe — a route's price, a peering's
fee, whether an identity is required — **is** binding, and is specified where it is observed: in the
packet-flow, payment, node-self-description and client-edge documents. A peer must learn that this hop
charges a fee and refuses an over-cap packet with `T04`. It must not have to learn that the fee is
spelled `fee` inside a table spelled `[[peers]]`.

What remains here is the operator's side of the same facts: what must be expressible, what must be
rejected, and when.

---

## 1. The contract

### 1.1 Loading

**CF-01** `[connector]` — A connector MUST load its configuration **once**, validate it completely,
and hold the result immutable for the process lifetime. A configuration that reaches the runtime has
already answered every question about presence, range and mutual consistency.
([ADR 0009](../adr/0009-one-typed-config-file-no-environment-layer.md))

**CF-02** `[connector]` — A connector MUST NOT take configuration from the environment. There is no
override layer, and no precedence model, because two configuration surfaces means a class of bug where
the deployed value is not the value anyone read.

**CF-03** `[connector]` — Reload is a restart. Anything that must change while running changes through
the operator surface, where the change is authenticated and audited.

**CF-04** `[operator]` — Secrets are referenced **by location** — a file path or a key-management
identifier — and never written inline.

**CF-05** `[connector]` — Conveniences MUST be resolved at load, so the runtime sees only primitives
and the packet path stays topology-blind.

### 1.2 Identity and signing

**CF-06** `[connector]` — A connector MUST be configurable with an identity key, and **one key serves
every purpose this connector signs for**: the key a packet is sealed to, the key its outbound claims
are signed with, and the key its self-description publishes. A second key minted for one surface is a
defect, not a feature.
([ADR 0018](../adr/0018-a-payload-is-sealed-to-the-terminating-connector.md),
[ADR 0050](../adr/0050-a-connectors-url-resolves-to-its-self-description.md))

**CF-07** `[connector]` — A connector MAY hold no identity key. It then cannot open a sealed payload
and MUST answer a termination it cannot open with an unsealed reject naming where to ask.
([ADR 0054](../adr/0054-an-unsealed-termination-reject-answers-where-to-ask.md))

### 1.3 Facts a node cannot introspect

**CF-08** `[operator]` — A connector MUST be configurable with its own **public** ILP address(es) and
its **public** client-edge endpoints, HTTP and BTP. A node cannot derive these: a container sees
`0.0.0.0:4000` and a private network, never `https://proxy.example/ilp`. Either endpoint MAY be
declared under any `peer_expose` (CF-17) — both listeners are served regardless, so a declared
endpoint is never refused as unexposed. Which may be **omitted** is `peer_expose`'s call, and a
connector MUST refuse to load, naming the key, when: `btp_endpoint` is absent and a BTP peer listener
is exposed; or `http_endpoint` is absent and **any** peer carriage is exposed, because a peer
covering a forward asks this node's client edge for claim-state over HTTP whichever carriage the
packet rides. A connector whose `peer_expose` is `"neither"` (the default) MAY be configured with an
address list and no endpoint at all — it still answers its self-description, unpeerable.
([issue #1220](https://github.com/toon-protocol/connector/issues/1220))

**CF-09** `[connector]` — These facts, and no others about software behind the connector, are what the
node self-description publishes. A connector describes **itself**.
([ADR 0050](../adr/0050-a-connectors-url-resolves-to-its-self-description.md))

### 1.4 Routes

**CF-10** `[operator]` — A route MUST be expressible as a prefix plus exactly one of:

- a **handler** and a **price** — the route terminates here; or
- a **peer** and a **price** — the route is forwarded to that peer.

A route MUST NOT carry a **fee**. A fee is what a connector retains for carrying one packet to a
counterparty, and that is the same work whichever prefix was addressed — so it belongs to the
peering, not to any route reaching it.
([ADR 0061](../adr/0061-a-fee-attaches-to-a-peering-not-to-a-route.md))

**CF-11** `[connector]` — A route that names both, or neither, MUST be refused at load. A terminated
route with no price MUST be refused: a route is never silently free.
([ADR 0020](../adr/0020-a-price-is-flat-and-attaches-to-a-handler.md))

**CF-12** `[connector]` — Two routes MUST NOT claim the same prefix, whatever their kind. App routes
and peer routes share one prefix namespace.

**CF-13** `[operator]` — A price attaches to a **handler**, and an operator charges differently for
different work by publishing a route per handler. A connector MUST NOT let one route's price vary with
what a packet **carries** — that is how it prices without ever interpreting what it carries. It MAY
vary with how **long** the packet's sealed payload is, which every hop can measure without opening it
([ADR 0065](../adr/0065-a-price-is-a-schedule-over-payload-length.md)).

**CF-13a** `[operator]` — A price MAY be written as a whole number, or as a table
`{ base = <n>, per_kib = <n> }` charging `base + per_kib × ceil(payload_len / 1024)` where
`payload_len` is the packet's own `data` length. The two spellings mean the same thing when the slope
is zero. A table MUST carry both keys: a connector MUST refuse one naming only `base`, by name, rather
than defaulting the slope to zero — a schedule meant to charge by size going out flat is silent
mispricing (ADR 0065, ADR 0009).

**CF-13b** `[connector]` — A connector MUST charge one figure per packet, computed from the arriving
`data` length, at every gate that charges: the client edge on either carriage, a peer arrival's
coverage check (CF-29), a probe's reject, and the termination. Computing a different figure at two
gates for one packet admits a packet across a peering that its termination then refuses, after the
covering claim is banked.

**CF-13c** `[connector]` — A connector that prices by size MUST publish the whole schedule wherever it
publishes a price: its self-description and its greeting carry the slope beside the base, so one free
read answers every payload size ([ADR 0011](../adr/0011-rejects-accumulate-fees-and-probes-discover-cost.md)'s
cacheability). A greeting's own `amount` remains what the greeted request costs.

**CF-13d** `[operator]` — A route MAY carry `request`, an arbitrary table naming what a client should
send to use it. A connector MUST validate only that the value **is** a table — never a key inside
it, and never `deny_unknown_fields` on its contents — and MUST publish it verbatim, unread, on that
route's self-description entry and on the greeting for that destination, omitted (not `null`) where
the operator wrote none. A connector MUST NOT fetch this fact from the app or any other source: an
operator declares it, or it is absent.
([ADR 0067](../adr/0067-a-route-declares-its-request-shape-and-the-connector-never-reads-it.md))

**CF-14** `[connector]` — Two routes naming the same handler MUST agree on its price, comparing whole
schedules: same base and same slope.

**CF-15** `[operator]` — A route MAY require a specific client transport. A connector that pins one
MUST publish the requirement **on that route's own entry** in its self-description
([ND-05a](self-description-spec.md), [ADR 0072](../adr/0072-a-carriage-pin-is-published-on-the-route-that-enforces-it.md));
enforcing a requirement it does not advertise is the defect that refused every relay publish on the
devnet fleet, twice — once because the announce carried no such key at all, and once because the
per-node field it was replaced with has nothing honest to say about a node that pins one of its own
addresses and not the other (TOON_Network#111).

### 1.5 Peerings

**CF-16** `[operator]` — A peering MUST be expressible as: a peer id, a **counterparty key**, a
carriage to reach it on, a fee, and a cap. A peering is created by an operator and by nothing else —
it cannot be bought, learned, earned, or announced into existence.
([ADR 0043](../adr/0043-purchasable-peering-is-removed.md), [ADR 0006](../adr/0006-the-connector-is-mechanism-not-policy.md))

**CF-17** `[connector]` — A peering's carriage is **BTP over `wss://`** or **ILP-over-HTTP over
`https://`**. A connector MAY expose both. Below the transport there MUST be one pipeline: a PREPARE
that arrived over HTTP is indistinguishable from one that arrived over BTP, and behaviour present on
one carriage and not the other is a defect rather than a property of the carriage.
([ADR 0027](../adr/0027-connectors-peer-over-btp-or-http-and-the-raw-tcp-peer-wire-is-deleted.md))

**CF-18** `[connector]` — A plaintext peer endpoint (`ws://`, `http://`) MUST be refused. A connector
MAY offer a **node-wide** opt-in for loopback and test use; it MUST NOT offer a per-peering one, which
would read as an ordinary property of that peering and be copied into production one peer at a time. A
node with the opt-in set MUST log every plaintext peering at startup.

**CF-19** `[connector]` — A cap MUST be expressible per peering, MUST have a default, and MUST be
greater than zero. A cap of zero is not a smaller cap; it is a peering that can carry nothing.

**CF-20** `[connector]` — A connector MUST NOT raise its own cap. The number comes from outside —
the configuration file, or a controller writing through the operator surface. A cap that grows with
demonstrated good behaviour is a trust mechanism, and trust is policy.
([ADR 0049](../adr/0049-the-cap-bounds-one-packet-is-discovered-by-t04-and-is-set-from-outside.md))

### 1.6 Channels

**CF-21** `[operator]` — A connector MUST distinguish, in configuration, three channel roles:

| role               | means                                                                           |
| ------------------ | ------------------------------------------------------------------------------- |
| **peer channel**   | which channel a peer's claims are judged against, and the key they verify under |
| **client channel** | which channel a client's claims are judged against                              |
| **pay channel**    | a channel this connector pays _from_, as a client of another node               |

**CF-22** `[connector]` — The **client** book MUST NOT share an id with either of the others: one
channel that is both a peer's and a client's, or both paid from and received on, is one channel
counted as credit twice, and MUST be refused at load. The **peer** and **pay** books MAY share one,
and a connector MUST NOT refuse it — holding a single channel with a single hop in both roles is the
deployed shape, the peer role judging what arrives and the pay role covering what this connector
sends, with exactly one book signing per packet. Ids are compared within a chain, and over each
chain's canonical form rather than over the operator's spelling.

**CF-36** `[connector]` — A row in **any** of the three books MUST be refused at load, by name, if
the connector configures no settlement for that row's own chain. The settlement configuration is
where the connector's on-chain identity on that chain comes from (CF-24, and there is no second key
— ADR 0030), so without it the connector is not a participant of the channel: it could verify the
claims and never redeem one, rendering carriage for money it cannot collect. The rule is per chain
and no wider, and it does not depend on which book the row is in — see
[`peer-carriage-spec.md` §11.1](peer-carriage-spec.md) for the per-book consequences and why the
client book's declared-channel latitude (CF-23's "a configured row", and the deposit-cap exemption)
does not reach it. ([issue #1138](https://github.com/toon-protocol/connector/issues/1138))

**CF-23** `[connector]` — A claim's signature MUST be verified against **this connector's own record
of the channel** — a configured row, or a channel resolved from chain — and never against anything the
claim declares about itself.
([ADR 0052](../adr/0052-permissionless-payment-is-guaranteed-and-a-claim-is-what-authorises.md))

**CF-37** `[connector]` — A peering MUST be bound to at least one **peer channel**, and a
configuration naming a peering with none MUST be refused at load. A peering with nothing to judge an
arriving claim against can never take the peer role at all — its counterparty is admitted as an
ordinary client instead, and the runtime symptom is silence, because the peering appears to work.
See [`peer-carriage-spec.md` §1.2](peer-carriage-spec.md) for the role decision this binding is half
of.

**CF-38** `[connector]` — A peering a **route forwards to** MUST be bound to a **pay channel**, and a
route naming a peering with none MUST be refused at load, naming both the route and the peering. A
connector covers every PREPARE it sends ([ADR 0042](../adr/0042-a-packet-carries-its-claim.md)), so a
forward with nothing to sign a covering claim from has no uncovered path left to fall back to and
would reject every packet on that route. The rule is keyed on **routes** and no wider: a peering this
connector only ever accepts from owes nothing and needs no pay channel.
([issue #1145](https://github.com/toon-protocol/connector/issues/1145))

**CF-39** `[connector]` — A connector that can **resolve a channel** MUST also be configured with a
durable location for its claim watermarks, and MUST be refused at load if it is not. It can resolve
one if it configures a channel in any of the three books, **or** if it configures settlement for any
chain — a settlement table is what lets an undeclared channel be resolved from chain and its claim
accepted (CF-27), so such a connector takes payment from senders it was never configured for. Price
is not the trigger and neither is a route: a claim presented against a free route is admitted the
same way, and it advances the same watermark. A connector that configures neither a book nor
settlement can resolve nothing, refuses every claim, and is exempt — that exemption is the point of
the rule's shape, because a requirement placed where it cannot bite is answered with a path nobody
checked.

Amended by [issue #1186](https://github.com/toon-protocol/connector/issues/1186). The rule read "a
channel in **any** of the three books" and missed the permissionless shape entirely — a priced route
and a settlement backend, declaring no channel — which is both the configuration an operator should
be running and the one most exposed to strangers. It MUST verify that location is writable at startup, naming the path when it is not; it
MUST replay what is already there before it serves; and it MUST refuse to start on a record it cannot
read or cannot decode, rather than starting at no watermarks. A watermark held only in process memory
is not a replay defence: after a restart every spent nonce reads as fresh, every claim a client has
already spent buys service again, and nothing in a log shows that it did.
([issue #605](https://github.com/toon-protocol/connector/issues/605))

### 1.7 Settlement

**CF-24** `[operator]` — Settlement MUST be configurable **per chain**, each with its own endpoint,
contracts, token and key.

**CF-25** `[connector]` — A connector MUST verify its settlement configuration against the chain at
startup and MUST refuse to boot on a disagreement — a token's decimals, a resolved contract. Nothing
downstream may then ask whether these facts are true.

**CF-26** `[connector]` — A fact the settlement backend already holds MUST NOT be declared a second
time elsewhere in the configuration. Two declarations of one fact is how a mainnet node comes to
announce itself as devnet.

### 1.8 Permissionless payment

**CF-27** `[connector]` — A connector MUST accept payment from a buyer it has never heard of, whose
channel it resolves from chain. Registration with the operator is never a precondition for paying.
([ADR 0052](../adr/0052-permissionless-payment-is-guaranteed-and-a-claim-is-what-authorises.md))

**CF-28** `[connector]` — A connector MUST bound the chain lookups an unidentified sender can cause,
and MUST refuse rather than serve when that bound is reached. The bound's existence and its refusal
behaviour bind; the numbers are policy. Without it, CF-27 asks an operator to absorb unbounded cost
from strangers.

**CF-29** `[operator]` — A client identity MUST be expressible, and MUST be optional. An identity
**identifies**; it authorises nothing and cannot substitute for a claim. An empty secret means the
identity is a name, not a credential.

### 1.9 The operator surface

**CF-30** `[operator]` — Read authority and write authority MUST be separately configurable. A
credential that can inspect MUST NOT thereby be able to mutate.
([ADR 0008](../adr/0008-operator-surface-splits-read-from-write.md))

**CF-31** `[connector]` — The operator surface MUST be omittable. A node configured without one
exposes no operator surface at all, rather than an unauthenticated one.

**CF-32** `[connector]` — A runtime-written peer or route MUST NOT take a key the configuration file
owns. A colliding write is refused outright.
([ADR 0034](../adr/0034-a-runtime-peer-route-table-never-shadows-the-config-file.md))

**CF-33** `[connector]` — On load, a durable runtime row whose key the configuration file owns MUST be
**deleted**, not shadowed, and the deletion MUST be recorded where an operator will see it. Ownership
is permanent, not a precedence that flips back when the key is removed.

### 1.10 What a connector must refuse

**CF-34** `[connector]` — Every rule above whose verb is _refuse_ is a **load-time** failure that names
what is wrong. A connector MUST NOT start with a configuration it has not fully accepted.

**CF-35** `[connector]` — A connector SHOULD refuse a **removed** configuration key by name rather than
ignoring it. This is a convention of this implementation rather than protocol law — it binds nobody
else, because nobody else has these keys — and it is what stops an operator's committed file silently
changing meaning under an upgrade.

---

## 2. This implementation's file

**Non-normative.** Everything below is how _this_ connector spells §1. A second implementation may
spell it however it likes.

### 2.1 Top level

| key                                        | type                                  | required | expresses                                            |
| ------------------------------------------ | ------------------------------------- | -------- | ---------------------------------------------------- |
| `client_edge_addr`                         | socket address                        | yes      | where the client edge binds — **not** its public URL |
| `[signer]`                                 | table                                 | yes      | CF-06, the one identity key                          |
| `[[routes]]`                               | array of tables                       | —        | CF-10 – CF-15                                        |
| `[[peers]]`                                | array of tables                       | —        | CF-16 – CF-20                                        |
| `[[peer_channels]]`                        | array of tables                       | —        | CF-21, the peer book                                 |
| `[[client_channels]]`                      | array of tables                       | —        | CF-21, the client book                               |
| `[[pay_channels]]`                         | array of tables                       | —        | CF-21, channels this node pays from                  |
| `[[client_identities]]`                    | array of tables                       | —        | CF-29                                                |
| `[settlement.evm]` / `[settlement.solana]` | tables                                | —        | CF-24, CF-25                                         |
| `[operator]`                               | table                                 | no       | CF-30, CF-31                                         |
| `[node]`                                   | table                                 | —        | CF-08, the facts a node cannot introspect            |
| `peer_expose`                              | `"neither"`/`"btp"`/`"http"`/`"both"` | no       | CF-17                                                |
| `peer_allow_plaintext_endpoints`           | bool                                  | no       | CF-18's node-wide opt-in                             |
| `socks_proxy`                              | `socks5h://` URL                      | no       | ADR 0070, the one onion dial path                    |
| `[[tokens]]`                               | array of tables                       | —        | ADR 0071, the tokens this node deals                 |
| `[[rates]]`                                | array of tables                       | —        | ADR 0071, one ordered pair's rate and guards         |
| `[rate_guards]`                            | table                                 | —        | ADR 0071, this node's dealing policy                 |
| `state_dir`                                | path                                  | CF-39    | where durable state lives                            |

**How the file is read.** Every table in it is `deny_unknown_fields`, so an unrecognised key — a typo,
or one from a shape this build does not implement — is a load failure that names it rather than a line
silently dropped. The only environment variable the binary reads is `RUST_LOG`, and it sets log
verbosity and nothing else (CF-02). [`deploy/connector-rust/connector.toml`](../../deploy/connector-rust/connector.toml)
is the annotated template. `*.toml` is the only configuration this binary has ever had: the retired
TypeScript connector's `*.yaml` (`nodeId`, `btpServerPort`, `adminApi`) is gone from the repository
entirely.

**One listener.** `client_edge_addr` is where `POST /ilp` and `GET /ilp/btp` are served, where the
operator surface is mounted when `[operator]` is configured, and where the peer carriages ride when
`peer_expose` selects any. There is no second port and no second bind address.

**`[signer]`, and every `key` table under `[settlement]`,** take exactly one of `key_file` or
`kms_key_id` — a location, never a value (CF-04).

**Routes.** A route is a `prefix` plus exactly one of `handler_url` or `peer_id`, and a price is
required on **both** branches, each with its own named refusal ([ADR 0028](../adr/0028-a-forwarded-route-is-priced-at-the-client-edge.md);
CF-10, CF-11). Write `price = 0` where free is deliberate. A price is either a whole number or a
`{ base, per_kib }` table charging by payload length (CF-13a) — `price = { base = 1000, per_kib = 30 }`
— and the two spellings are one value when the slope is zero. `transport` is meaningful only alongside
`handler_url` (CF-15). `request` (CF-13d) is an optional arbitrary table, published unread wherever
the route's price is published; unlike every other row in `[[routes]]`, its contents are not
`deny_unknown_fields` — that guarantee stops at the row, not inside a blob whose keys are the app's
business.

**Peerings.** A peer row carries an `id`, an optional `endpoint` whose scheme selects the carriage, a
`max_packet_amount` (CF-19's cap — `0` is refused by name, and there is no disabling spelling) and a
`fee` (CF-16). A row with no `endpoint` is accept-only; a row with neither an `endpoint` nor a
`peer_expose` for it to be dialled into is refused, because it can never establish. Nothing on the row
authenticates the peering: [ADR 0060](../adr/0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md)
deleted the shared secret outright, and the role is proved by the peer-channel binding of CF-37 plus a
verified claim signature. A peering may also be established while the process serves, from the
counterparty's URL, over the operator surface
([ADR 0058](../adr/0058-a-peering-is-established-from-a-url.md)); CF-32 and CF-33 govern what such a
row may not take.

**`peer_expose` opens no port.** It selects which peer carriages are handled on the listener this node
already serves ([`peer-carriage-spec.md` §2.1](peer-carriage-spec.md)) — this connector has no
dedicated peer listener. A node that leaves it at its `"neither"` default still serves clients over
both client transports, and a node that sets it still serves an anonymous client that presents no
identity at all (CF-27). It is also the one peering fact the node self-description publishes: which
carriages exist, never who rides them.

**`socks_proxy` is one proxy, selected by host.** It names the single SOCKS5 proxy an onion endpoint
is reached through ([ADR 0070](../adr/0070-an-onion-address-is-a-host-not-a-carriage.md)); which dials
take it is read off the endpoint's own host — a host ending in `.onion` or `.anyone`, the two TLDs
the `anon` daemon has published (issue #1284) — so there is no per-peer
`proxy` key and no all-outbound mode, and nothing in this file states it a second time. It covers the
ILP wire, and settlement RPC only where a settlement table opts in (below); a route's `handler_url`
is outside its scope. The value must name a
host — `socks5h` is not a _special_ URL scheme, so `socks5h://` and `socks5h:9050` both parse and
neither is a proxy address, and both are refused by name. The scheme must be
`socks5h://` and every other scheme is refused by name at load, because a `socks5://` proxy resolves
the hostname locally, no local resolver resolves a hidden-service name, and a node that accepted one
would fail its onion peerings at dial time instead of at the line an operator wrote.

**The three channel books** (CF-21) are told apart by what each does with a claim. `[[peer_channels]]`
is the channel a peering's claims are **judged against**, and names the `counterparty_key` whose
signature is accepted on it; `[[client_channels]]` is a channel this node **receives** claims on, and
names its `counterparty` the same way (CF-23); `[[pay_channels]]` is one this node **pays** from — the
channel every PREPARE forwarded to that peer carries a covering claim on
([ADR 0042](../adr/0042-a-packet-carries-its-claim.md)). An EVM row in any book names the channel by
`channel_id` and its EIP-712 domain by `chain_id` and a token network
([ADR 0024](../adr/0024-peer-wire-claims-sign-the-eip-712-balance-proof.md)); a Solana row names a
`channel_account` instead and carries neither, because Solana has neither a numeric chain id nor a
per-token verifying contract for a row to name. No row in any book names a `program_id`: the
settlement program [ADR 0053](../adr/0053-a-solana-claim-binds-its-domain-the-way-an-evm-claim-does.md)
binds into a Solana claim is `[settlement.solana]`'s and is read from nowhere else (CF-26), and the
field survives on each Solana row solely to be refused by name. A Solana `[[pay_channels]]` row must
additionally name a channel the same peering binds as a Solana `[[peer_channels]]` row, and is refused
at load if it does not: `programId` is a required field of the Solana claim wire, where an EVM claim's
domain fields ride optional, and both peer carriages render it from that peer-channel row.

**A pay-from row's `client_edge_url`** is that peer's own `POST /ilp` — where this node arrives as an
ordinary buyer, and where it asks `POST /ilp/claim-state` where its claims on the channel stand. The
nonce and the cumulative amount are never remembered here and never guessed, because the receiver is
the authority on its own watermark. The signing key is that row's chain's settlement key,
`[settlement.evm]`'s or `[settlement.solana]`'s, and there is no second key to configure (CF-24,
[ADR 0030](../adr/0030-an-operator-announces-a-node-the-node-still-does-not.md)).

**Settlement has two shapes** (issue #628). The legacy flat one — `chain`, `rpc_url`,
`contract_address`, `token_address`, `decimals` and a key table, all directly under `[settlement]` —
is **frozen at `chain = "evm"`** and never accepts `"solana"`; a node settling on Solana, or on both
chains at once, writes the keyed shape in §2.1's table instead. `contract_address` is the
**`TokenNetworkRegistry`**, the contract `getTokenNetwork(token)` is called on, and not a channel
contract; `[settlement.solana]` names the deployed `payment-channel` `program_id` in its place. Mina
is not a settlement chain in either shape
([ADR 0002](../adr/0002-drop-mina-from-the-rust-connector.md)), and is no longer in this
repository at all ([ADR 0065](../adr/0065-mina-leaves-the-repository.md)). An absent `[settlement]` is legal, and
every channel operation then answers `503`; a present but wrong one is a startup failure, because a
real backend is constructed for every chain configured before the node serves anything (CF-25).

**Accepting x402 batch-settlement channels is opted into per chain** ([ADR 0074](../adr/0074-a-client-may-pay-over-an-x402-batch-settlement-channel.md)
decision 1), by writing a `batch_settlement` sub-table under that chain's keyed settlement table. It is
**off unless written**: there is no `enabled` key, because the table's presence already says so, and
the frozen legacy `[settlement]` shape has no such sub-table. Everything decision 2 fixes about an
admissible channel comes from the enclosing table and is not declared again (CF-26): the receiver (or
Solana sponsor) is that table's settlement key, and the token or mint is its `token_address`. What the
sub-table holds is only the terms that are this node's to choose. The first column is the chain whose
`[settlement.<chain>.batch_settlement]` takes the key — these are not top-level keys, which is why the
table does not start with one:

| chain  | key                       | default              | refused by name when                                            |
| ------ | ------------------------- | -------------------- | --------------------------------------------------------------- |
| EVM    | `min_withdraw_delay_secs` | `86400` (a day)      | below `900`, or above `2592000` (the contract's 30-day maximum) |
| EVM    | `contract_address`        | x402's `0x4020…0003` | not a 20-byte address                                           |
| EVM    | `asset_eip712_name`       | **required**         | empty                                                           |
| EVM    | `asset_eip712_version`    | **required**         | empty                                                           |
| Solana | `min_grace_period_secs`   | `86400` (a day)      | below `900`                                                     |
| Solana | `min_sponsored_deposit`   | **required**         | `0`; it bounds a public endpoint that spends this node's rent   |
| Solana | `program_id`              | `CHNLx…yGsX`         | not a base58 32-byte id                                         |

The two minimums are published in the greeting, and a channel whose `withdrawDelay` or `grace_period`
falls short of them is not admitted. The floor of 900 seconds is x402's own; the day is the window a
delayed `claim` or `settle_and_seal` still has to land in. `contract_address` and `program_id` default
to the one address x402 deploys on each chain's test and main networks alike, and are the voucher
domain the connector reads from its config and never from a voucher (decision 4). The Solana
`program_id` here is x402's `payment-channels`, not `[settlement.solana] program_id`, which is TOON's.

**`asset_eip712_name` and `asset_eip712_version`** are the EIP-712 domain `name` and `version` of
`[settlement.evm] token_address` -- `"USDC"` and `"2"` for the devnet's Circle FiatToken v2.2 -- and
are published on the greeting's `batch-settlement` `accepts[].extra` (ADR 0074 decision 8) so a stock
client can sign its deposit's ERC-3009/permit2 authorization under the asset's real domain. Configured
rather than read off the chain: an arbitrary ERC-20 need not expose an EIP-712 `version()` the way
Circle's FiatToken does, and this connector never itself signs or verifies under either value, so
there is nothing here to prove against a live contract the way `decimals` is (issue #1345). Both are
required as soon as `[settlement.evm.batch_settlement]` exists -- there is no safe default for an
arbitrary settlement token, and publishing the wrong domain would build a deposit signature that
never verifies. Solana carries no equivalent key: the x402 SVM scheme's asset transfer has no EIP-712
domain of its own to publish.

**`decimals` is a declaration, not a conversion.** Nothing scales by it: every amount on the value path
— a route's price, a claim's amount, a channel's deposit — is already in the settlement token's base
units, and stays in the units of the leg it is on. Where a forward's two legs hold different tokens the
scale difference is folded into the **declared rate**'s ratio and taken from there
([ADR 0071](../adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md) decision 4), never
derived from this key: scale is not price, and nothing about a market lives in a token's metadata. It
is checked instead, against the token's own `decimals()` at startup, and a disagreement names both and
refuses to boot (CF-25). Zero is refused outright.

**Declaring a denomination is optional, and declaring none costs nothing.** `[[tokens]]`, `[[rates]]`
and `[rate_guards]` are how an operator says which tokens their node **deals**, what each is worth
against one **numeraire**, and under what guards
([ADR 0071](../adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md) decisions 3 and 5). A
node that writes none of the three loads, serves and forwards exactly as it did before they existed.
A `[[tokens]]` row names its token as `evm:<contract>` or `solana:<mint>`, may set `numeraire = true`
— exactly one row may, and two is refused by name — and may carry a `quote`: one or two pools on that
token's **own settlement chain**, each with its own `twap_window_secs`, the last ending at the
numeraire. A quote is read over its chain's `[settlement.<chain>]` RPC endpoint, so a quote on a chain
with no such table is refused at load rather than becoming a poller that can never take a reading; and
a path that ends anywhere but the numeraire is refused too, because a cross rate is composed as
`(X/numeraire) ÷ (Y/numeraire)` and a leg answering in another unit would be composed as though it had
not. A `[[rates]]` row is keyed by an **ordered** pair — `from` and `to`, never sorted, because
direction is the trade — and carries a `rate = { numerator, denominator }` over base units, guard
overrides, or both; a row naming a token no `[[tokens]]` row declares is refused, and a rate with a
zero half is refused in the domain's own words with the row named. `[rate_guards]` states `spread`,
`ttl_secs` and `max_move` once, and is **required as soon as anything can produce a rate** — none of
the three has a safe default, and a defaulted spread is dealing at mid. `spread` and `max_move` are
`{ numerator, denominator }` fractions rather than percentages or basis points, for the reason
[ADR 0010](../adr/0010-flat-per-packet-fee-and-minimum-delivery.md) deleted the basis-point fee:
there are no floats on this path, and an operator dealing at half a basis point writes `1/20000`
rather than watching it round to zero. Each is validated by the same constructor the rate table
builds one with, so config and the guards themselves can never disagree about what a guard is — a
`max_move` of `0/1` is _pinned_, a coherent declaration about a par pair, and is not refused.
Declaring tokens alone produces no rate and needs none, which is what a same-asset cross-chain hop
declares.
Nothing here is an environment variable and nothing is mutable: the file declares the _source_, and
what a refresh changes is an observation.

**`[[client_identities]]`.** Each entry is an `id` a request presents in `ILP-Peer-Id` and the `secret`
it must present in `Authorization: Bearer <secret>`; an empty or omitted secret makes that identity a
name rather than a credential, and the header may then be absent (CF-29). An empty `id`, or a
duplicated one, is refused at load. Configuring none of these is not a closed door: a request
presenting no `ILP-Peer-Id` is anonymous, which is a first-class path (CF-27), and a node with no
entries serves clients exactly as it did before the section existed. What the section changes is that
an `ILP-Peer-Id` presented and _not_ authenticated is refused `401`, answered before the route is
looked up ([`client-edge-spec.md` §1.2](client-edge-spec.md)).

**`state_dir`** is CF-39's durable location. Two append-only journals live there:
`client-edge-claims.log`, the claims accepted at `POST /ilp`, and `peer-claims.log`, the peer
carriage's own claim book. In a container it MUST be a **mounted volume** rather than a path in the
writable layer — a watermark that dies with the container is the same defect one indirection down. The
image runs as uid `10001`, so a named volume, whose ownership follows, is simpler than a host bind
mount, which has to be `chown 10001:10001`ed first.

### 2.2 Local operational knobs

Visible to nobody outside the process, and **not** part of §1. They shape this connector's own resource
use and belong in an operator's guide rather than a protocol specification.

`channel_liveness_ttl_secs` · `channel_serve_stale_secs` · `channel_reattempt_interval_ms` ·
`unresolvable_lookup_budget_per_signer` · `unresolvable_lookup_budget_total` ·
`unresolvable_lookup_budget_window_secs` · `unresolvable_lookup_budget_max_wait_ms` ·
`btp_session_window`

**`rpc_via_socks_proxy` sends one settlement table's RPC through `socks_proxy`**
([ADR 0073](../adr/0073-settlement-rpc-may-ride-the-circuit-once-every-wait-on-it-is-bounded.md)). It
is a boolean on `[settlement.evm]` and on `[settlement.solana]`, `false` when omitted, and not
accepted by the frozen legacy `[settlement]` shape. When a table sets it:

- every client of that table's `rpc_url` dials through the node's one `socks_proxy` as `socks5h`: the
  settlement backend, and on EVM the channel-index syncer and the rate source too. None is left
  direct, and a proxy that is down is a failed call, never a direct dial;
- each chain rides its own circuit, pinned by a fixed SOCKS username (`toon-settlement-evm`,
  `toon-settlement-solana`), which relies on the daemon's `IsolateSOCKSAuth` (on by default);
- a node with no `socks_proxy` is refused at load (`SettlementRpcViaSocksProxyWithoutProxy`), since
  the key selects the node's one proxy and never names a second;
- a plain `http://` `rpc_url` is refused at load (`SettlementRpcViaSocksProxyPlaintext`) unless its
  host is a `.onion` or `.anyone` address, because an exit relay could otherwise read and rewrite
  every answer, a channel's deposit and a transaction's receipt included.

It is an opt-in and not read off the host, unlike a peer endpoint's proxy, because a public RPC's
host carries no signal the way an onion host does: a node whose RPC is a self-hosted node on a
private network must keep dialing it direct. The circuit hides the node's address from the RPC
provider and nothing else: the provider still sees every query and transaction, and an API-keyed
endpoint ties them to the account that holds the key. Every settlement client, proxied or not, now
runs under the same bounds: 20s to connect, 30s per request, idle connections dropped after 30s, and
a 403 or 429 retried with backoff.

`[settlement.evm]` carries two more of the same kind (issue #661), for the local channel index a node
builds from its own `TokenNetwork`'s logs so that resolving an unfamiliar channel is a map hit rather
than an RPC call: `channel_index_from_block`, the block a cold start with no checkpoint backfills from
— it defaults to `0`, so an operator who knows their `TokenNetwork`'s deploy block should set it
rather than scan a public chain from genesis — and `channel_index_confirmations`, how many blocks
behind head a log must be before the index applies it. That one defaults to `5`, and `0` is refused at
load, since there is deliberately no reorg-unwind path. Omitting both changes no behaviour: a channel
the index has not caught up to falls through to a direct chain read.

`btp_session_window` splits, and shows the general shape: **the existence of an in-flight limit and
what a connector does when it is exceeded are law** (client-edge specification); the number that sets
it is not. _The limit is law, the number is policy._

### 2.3 Tombstones

Parsed **solely to be rejected by name**, per CF-35. Finding one of these identifiers in the tree is
finding a tombstone, not a live mechanism.

| key                                     | removed by                                                                                                                                                                                                                            |
| --------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `peer_wire_addr`                        | [ADR 0027](../adr/0027-connectors-peer-over-btp-or-http-and-the-raw-tcp-peer-wire-is-deleted.md)                                                                                                                                      |
| a peer's `addr`                         | [ADR 0027](../adr/0027-connectors-peer-over-btp-or-http-and-the-raw-tcp-peer-wire-is-deleted.md) (#679) — the `SocketAddr` form of the same removal; a peer is reached by `endpoint` now                                              |
| a Solana channel row's `program_id`     | (#1082, #1128, #1146) — the program is `[settlement.solana]`'s, per CF-26; the field is spelled out on each Solana row only so writing one is named rather than lost in a shape mismatch                                              |
| `ceiling`, `flush_interval_ms`          | [ADR 0033](../adr/0033-the-exposure-machinery-is-retired-not-restated.md)                                                                                                                                                             |
| `[peer_sale]`                           | [ADR 0043](../adr/0043-purchasable-peering-is-removed.md)                                                                                                                                                                             |
| `apex`, `[[children]]`                  | [ADR 0009](../adr/0009-one-typed-config-file-no-environment-layer.md)'s update (#1057)                                                                                                                                                |
| `claim_enforcement`                     | [ADR 0042](../adr/0042-a-packet-carries-its-claim.md) item 4 (#1062 decided, #1077 deleted)                                                                                                                                           |
| a peer's `credential`                   | [ADR 0060](../adr/0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md) (#1157) — a claim proves a peering, so there is no shared secret to write                                                                        |
| a route's `fee`                         | [ADR 0061](../adr/0061-a-fee-attaches-to-a-peering-not-to-a-route.md) (#1159) — it moved to the `[[peers]]` row the route's `peer_id` names                                                                                           |
| `[announce]` and its announce-only keys | [ADR 0046](../adr/0046-the-kind-10032-announce-is-removed-a-connector-needs-no-relay.md) (#1074); the section's three surviving fields are `[node]`, per [ADR 0050](../adr/0050-a-connectors-url-resolves-to-its-self-description.md) |

---

## 3. Consistency

This document uses exactly the vocabulary of [`CONTEXT.md`](../../CONTEXT.md) and implements
[ADR 0009](../adr/0009-one-typed-config-file-no-environment-layer.md),
[ADR 0034](../adr/0034-a-runtime-peer-route-table-never-shadows-the-config-file.md) and
[ADR 0047](../adr/0047-the-configuration-schema-is-implementation-detail-capabilities-are-law.md).

**Coverage:** none of CF-01 – CF-39 is vectored, and none ever will be. Configuration is not a wire
surface — you cannot express "this key is refused by name" as a byte fixture — so per
[ADR 0045](../adr/0045-a-behavioural-rule-is-normative-prose-until-its-vector-lands.md) these rules are
prose-normative **permanently**, not provisionally, and do not enter the debt ledger. What _is_
vectorable is the observable consequence of a configuration — a price answered, a `T04` refused — and
that belongs to the documents where those are specified.

**Not yet built**, and marked so rather than described in the present tense: CF-33's load-time
reconciliation is #1076; CF-20's runtime-settable cap is #1079; the `apex`/`[[children]]` tombstone
is #1075. (`claim_enforcement`'s tombstone was #1077 and has landed; `[node]` (§2.1) —
[ADR 0050](../adr/0050-a-connectors-url-resolves-to-its-self-description.md)'s rename of
`[announce]` — landed with #1080, and `[announce]` itself is now a tombstone refused by name
alongside its keys.)
