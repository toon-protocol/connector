# Operating a connector

**Status:** **Normative for its numbered rules**, of which there are deliberately few — see "What
binds, and what does not" below. Per [ADR 0045](../adr/0045-a-behavioural-rule-is-normative-prose-until-its-vector-lands.md)
these are prose-normative permanently rather than provisionally: the operator surface is not a wire
surface, so it is not vectorable and does not enter the debt ledger.

**Consumers:** anyone running this connector, and anyone writing a second one who wants to know how
little of this binds them.

**Vocabulary:** [`CONTEXT.md`](../../CONTEXT.md). MUST, MUST NOT, SHOULD, MAY per RFC 2119.

---

## What binds, and what does not

**Almost nothing here is protocol law, and that is the finding rather than a gap.**

Apply [ADR 0047](../adr/0047-the-configuration-schema-is-implementation-detail-capabilities-are-law.md)'s
test — _can a counterparty observe it?_ — to the operator surface, and the answer is no. A second
connector may expose no operator surface at all, driven entirely by its configuration file, and still
be a conforming TOON node. [ADR 0008](../adr/0008-operator-surface-splits-read-from-write.md), which
decides this surface, is scoped _"connector architecture — internal to this codebase"_, and that scope
is correct.

What **is** law is the **effect** of an operator's actions, and each is specified where it is
observed: a route's price in the client-edge document, a peering's fee and cap in the peering
document, a channel's existence on chain. An operator changes the world; the surface they change it
through is their implementation's business.

So this document is mostly an **operator's guide**, with a small normative core in §3. That is the
honest shape, and stating it is more useful than manufacturing rules to fill a section.

---

## 1. Zero to a serving node

The path no runbook covered, because each of the sixteen in `docs/operators/` starts from somewhere
further along.

### 1.1 What a node needs before it serves anything

**Three things, and only the first is mandatory.**

1. **An identity key.** 32 raw bytes, or 64 hex characters, in a file the connector reads at boot.
   This one key is everything the node signs for — the key a packet is sealed to, the key its outbound
   claims carry, the key its self-description publishes. There is no second key for a second surface.
2. **At least one route**, or the node serves nothing. A terminated route needs a handler and a price;
   a forwarded route needs a peer and a price. What the forwarding hop keeps is the **peering's** fee,
   which is configured on the peering and not on the route.
3. **A settlement backend**, if the node is to be paid on chain. A node with none can still route and
   still terminate — it simply cannot verify a claim against a chain, so it serves only channels its
   configuration names.

**Fund the settlement key _before_ you configure the backend.** The third item reads as a
configuration step and it is not one. A Solana backend's `connect` **submits** a transaction — a
`create_associated_token_account_idempotent` for its own key — and then simulates an
`InitializeChannel` against a throwaway counterparty to prove the configured `program_id` really is
the payment-channel program. Both need that key to hold native SOL. Against an unfunded key the node
stops with **exit 1** on an RPC error that names no configuration key at all, so it reads like an
unreachable endpoint or a typo rather than an empty account. The EVM backend only reads at `connect`,
so this is the Solana leg specifically. (From the only third-party bring-up, issue #1098.)

Everything else — peerings, an operator surface, client identities — is added when there is a reason.

### 1.2 The smallest configuration that serves

```toml
client_edge_addr = "0.0.0.0:3000"

[signer]
key_file = "/app/secrets/signer.key"

[[routes]]
prefix = "g.example.app"
handler_url = "http://app:3100/write"
price = 1000
```

That is a complete node: it terminates one priced route, answers a greeting to anyone who asks, and
accepts a claim on any channel it can resolve. It settles nothing, because it has no settlement
backend — which is a legitimate configuration, not a degraded one.

**Note `handler_url` carries a path, not a bare origin.** An envelope's target resolves _beneath_ it,
and a target of `/` against a bare `http://app:3100` resolves back to the origin and 404s. The 404
then rides home as a **FULFILL**, not a reject — the app answered, and the answer was paid for
([ADR 0051](../adr/0051-a-reject-code-binds-where-a-sender-must-act-differently.md)). An operator who
expects a refund for a misrouted target will not get one, and this is the line that causes it.

### 1.3 It refuses to start when

A configuration that reaches the runtime has already answered every question about it
([ADR 0009](../adr/0009-one-typed-config-file-no-environment-layer.md)), which means every problem
below is a **boot failure naming what is wrong**, not a runtime surprise:

| refusal                                                                | why                                                                              |
| ---------------------------------------------------------------------- | -------------------------------------------------------------------------------- |
| the TOML does not parse, or a key inside a section is misspelled       | a typo must not read as an omission                                              |
| no configuration path was given                                        | there is no default and no environment fallback                                  |
| the signer key file is missing, or holds invalid material              | the node cannot sign anything                                                    |
| a **terminated** route has no `price`                                  | a route is never silently free                                                   |
| **any** route sets a `fee`                                             | a fee attaches to a peering, not to a route (ADR 0061)                           |
| a **forwarded** route has no `price`                                   | it is priced at this connector's own client edge                                 |
| a forwarded route names a peer id no `[[peers]]` row configures        | the routing table _is_ the relationship set                                      |
| `[[client_channels]]` is configured with no `state_dir`                | a restart would hand every spent claim back as free service                      |
| `state_dir` cannot be written                                          | same, discovered at boot rather than at the first claim                          |
| the claim journal is corrupt                                           | claims are the source of truth; a damaged journal is not recoverable by guessing |
| the operator surface is enabled with no bearer token, or no write keys | an unauthenticated operator surface is worse than none                           |
| a settlement block is present but unsatisfiable                        | a present-but-broken backend is a hard failure, never a silent degrade           |
| a **removed** configuration key is set                                 | see the configuration specification's tombstones                                 |

The last two are the ones that surprise operators. A settlement block that cannot reach its chain, or
whose token reports different decimals than configured, stops the node — deliberately, because the
alternative is a node that looks healthy and cannot be paid.

**Boot is not a dry run.** Everything above makes it tempting to start a node purely to find out
whether a configuration is good, and that habit is safe right up to the point where a **funded**
settlement key is in the file. It is not safe then: the Solana backend proves itself by submitting a
transaction, not by reading (§1.1), so an ephemeral container started to see whether a TOML parsed
pays a real fee on a real chain and leaves a real transaction behind. The bring-up behind issue #1098
lost 15,000 lamports to exactly that check. There is no dry-run verb to reach for instead — the binary
serves or it sends — so a settlement-bearing configuration can only be validated by booting it, and
booting it spends. Rehearse one against a disposable chain, not against the chain it will settle on.

### 1.4 Confirming it works, in order

1. **`GET /ilp`** — the node's self-description, and a plain `curl` is the whole of the test: it is
   free, unauthenticated, and needs no packet, encoder or protocol knowledge. If it answers, the node
   is up, its configuration loaded, and everything a stranger needs to pay it is in one document
   ([ADR 0050](../adr/0050-a-connectors-url-resolves-to-its-self-description.md),
   [`self-description-spec.md`](self-description-spec.md)).
2. **`POST /ilp` with no claim**, to a priced route — the greeting. Confirms the route exists, is
   priced, and quotes terms.
3. **A paid packet** end to end. Confirms the claim gate, the app delivery and the fulfilment.
4. **`GET /metrics`** on the operator surface, if one is configured.

Step 1 before step 2 is deliberate: a greeting tells you about one route, and the self-description
tells you about the node. It is also the step that carries over to §1.5 — the URL you just curled is
the entire thing you hand a counterparty who wants to peer with you.

### 1.5 Adding a peering

A peering is created by an **operator** and by nothing else. It cannot be bought, learned, earned or
announced into existence ([ADR 0043](../adr/0043-purchasable-peering-is-removed.md)).

**One write, and the URL is the whole of what you give it**
([ADR 0058](../adr/0058-a-peering-is-established-from-a-url.md)):

```
POST /peers { "id": "...", "url": "https://…/ilp", "fee": 100, "max_packet_amount": 5000,
             "deposit": 100000 }
```

The node `GET`s that URL's self-description and takes from it the endpoint, the carriage that
endpoint's scheme selects (`wss://` for BTP, `https://` for HTTP), the counterparty's edge identity,
and its per-chain facts. **On either chain a peering is two one-way x402 channels**
([ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md) decision 4,
#1378 on EVM, #1379 on Solana): the node opens and funds **only its own** outbound channel toward the
counterparty — an `x402BatchSettlement` channel on EVM, a `payment-channels` channel on Solana — on
the terms its `batchSettlements` entry publishes, with `deposit` (in base units of the shared token),
and binds the counterparty's channel toward it by the voucher signer its `voucherSigners` entry
publishes (EVM `payerAuthorizer`, Solana `authorized_signer`). **On Solana the `open` is posted to
the counterparty's `sponsorEndpoint`**, which co-signs, submits and admits it as `payee` and
`rent_payer` (ADR 0075 decision 3), so the counterparty can always land this node's latest voucher
with `settle_and_seal`, even after this node requests a close; the post leaves on `socks_proxy` when
the sponsor is an onion host (`connector_config::is_onion_endpoint`, ADR 0070). The counterparty
does the same with this node's URL; neither operator exchanges a channel id, and every forward to the
peer then carries a voucher on this node's channel. `deposit` is needed only when this node has no
open channel toward the counterparty yet; a repeat finds that channel and spends nothing (top it up
with `POST /channels/:id/fund`).

**A runtime peering written before ADR 0075** — an EVM one naming a `TokenNetwork` channel (before
#1378), a Solana one naming a channel of TOON's own payment-channel program (before #1379) — names a
channel this build cannot sign on, and the node refuses to boot on it, by name, pointing at ADR
0075's drain procedure — never dropping the row.

**There is nothing else to exchange out of band, and there is no shared secret.** Both halves an
earlier bring-up had to hand over by hand are gone rather than merely documented. No **channel** id
is copied between operators: each node opens its own and binds the other's by a published key
(ADR 0075). The **peer credential** that had to be byte-identical in both data dirs is
**deleted, with nothing replacing it**: a peer's role is proved per frame by a voucher (or, on a
packet that moves no value, a claim-state challenge) signed by the voucher signer bound to the
peering — the one its self-description publishes, or the one a `[[peer_channels]]` row names
([ADR 0060](../adr/0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md), as
[ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md) decision 5
amends it, issue #1380).
`[[peers]].credential` is a tombstone, so a configuration copied from an older runbook is refused by
name at boot rather than quietly ignored.

**What you supply, and why only these.** `id` is your own **local label** — never derived from the
peer's ILP address, which is self-asserted, and never from the URL host. `fee`, `max_packet_amount`
and `deposit` are your policy about that counterparty, and are in the request precisely because no
document can supply them.

**Whatever the URL serves is who the peering is with.** The identity in the document is not checked
against anything you sent, and it is not pinned, verified or attested by anything. It is
trust-on-first-use over TLS, and your vetting of the URL is the whole of the assurance. A party who
controls that hostname's DNS, or a certificate for it, chooses the counterparty — and so the receiver
this node funds a channel toward.

**This write can spend gas.** It is safe to retry: a repeat against a peering already established
finds the same channel and succeeds — on EVM this node's own open channel toward the counterparty,
and an open journaled but not yet confirmed is resumed rather than sent twice. The answer says which branch it took —
`"channel": { "id": "0x…", "status": "found" | "created" }` — so an unintended second channel shows
up in your own output rather than on a block explorer later.

**Two nodes that settle on more than one chain in common** must say which: add `"chain": "evm"` or
`"chain": "solana"`. Without it the write is refused by name rather than resolved silently.

**A new peering starts at a conservative cap.** The cap is the most this connector is willing to lose
in one theft, and nothing raises it automatically — a connector never earns its own cap
([ADR 0049](../adr/0049-the-cap-bounds-one-packet-is-discovered-by-t04-and-is-set-from-outside.md)).
Raising it is an operator decision, or a controller's: post the same `id` again with a larger
`max_packet_amount`. Omitting it, or writing zero, keeps the standing bound; nothing on this surface
removes one.

**A route through the peering is a second, separate write** — `POST /routes/peers { prefix, peer_id,
price }` — because a peering and a route are different decisions and one may exist without the other.
Onboarding is those two calls, with `POST /channels` still available for an operator who wants to open
a channel on their own terms first; this write then _finds_ it.

**`DELETE /peers/:id` is the kill switch.** It takes the carriage away with the durable row, so it is
immediate and needs no restart. On EVM it also unbinds the peer's voucher signer and stops signing on
this node's outbound channel, which stays open for the operator to withdraw from
(`POST /channels/:id/withdraw`). A peering still referenced by a runtime route is refused until the
route goes ([ADR 0034](../adr/0034-a-runtime-peer-route-table-never-shadows-the-config-file.md)).

A peering written in the config file is the same object, differing only in where it is recorded and
which wins a collision: config always wins, by refusing the runtime write outright.

### 1.6 There is no announce

A connector does not publish itself, and there is no mechanism by which it could
([ADR 0046](../adr/0046-the-kind-10032-announce-is-removed-a-connector-needs-no-relay.md)). An
announce assumes a Nostr relay exists, a connector fronts it, and a channel funds the write — and a
network of pure connectors has none of that.

**Being found is not the connector's job.** A node answers what it is asked, and copying those answers
into a discovery network is a **controller's** business — outside the connector by definition, and now
outside it in fact.

---

## 2. Running one

### 2.1 What changes without a restart, and what does not

| change                                          | how                                                               |
| ----------------------------------------------- | ----------------------------------------------------------------- |
| a route's price, a peering's fee, a handler URL | **edit the file and restart** — reload is a restart               |
| a leased route                                  | pushed by a **controller**, expires unless renewed, never durable |
| a runtime peer or peer route                    | written through the operator surface, **durable** across restarts |
| a channel: open, fund, withdraw, land           | the operator surface, against a settlement backend (§2.2)         |
| originating a packet outward                    | the operator surface                                              |

**A runtime row can never take a key the configuration file owns.** A colliding write is refused
outright, and on the next boot a runtime row whose key the file has since claimed is **deleted**, not
shadowed — ownership is permanent rather than a precedence that flips back
([ADR 0034](../adr/0034-a-runtime-peer-route-table-never-shadows-the-config-file.md)).

### 2.2 Channels

Every channel is an x402 `batch-settlement` channel, and a channel moves value **one way**
([ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md)). A node is the
payer on its **outbound** channels and the receiver on its **inbound** ones, and the surface treats
the two differently: an operator opens, funds and withdraws from what it pays on, and lands what it is
paid on. There are four writes (decision 11), each RFC 9421-signed like every other; a bearer token
performs none of them.

- **`POST /channels { terms, deposit, url? }`** opens an outbound channel. `terms` is the
  counterparty's `batchSettlements` entry for one chain, exactly as its self-description publishes it;
  `deposit` is the opening deposit in base units; `url` is the counterparty's URL, which a Solana
  `sponsorEndpoint` published as a path is resolved against. The node builds a channel the counterparty
  admits by its own published rules — its settlement key as payer and voucher signer, the counterparty
  in every receiving seat, the counterparty's minimum delay, a fresh salt — and deposits from its own
  settlement account: on EVM a `deposit` it sends itself, on Solana an `open` it signs and posts to the
  counterparty's sponsor endpoint. Terms on a network other than this node's, or naming a
  `receiverAuthorizer` other than `payTo`, are refused `400` before anything is built.
- **`POST /channels/:id/fund { amount }`** tops an outbound channel up by an **increment**, never a
  total.
- **`POST /channels/:id/withdraw`** winds an outbound channel down. It is one lever with two steps,
  and the chain decides which: on an open channel it **starts** (EVM `initiateWithdraw`, Solana
  `request_close`), and once due it **finishes** (EVM `finalizeWithdraw`, Solana `distribute`),
  answering `step: "started" | "finished"`. Between the two the counterparty has the channel's delay —
  one day by default — to land the latest voucher it holds, and whatever it lands is its own. Called
  before it is due, it answers `409` with the seconds left.
- **`POST /channels/:id/land`** lands the latest voucher this node holds on an inbound channel now: EVM
  `claim`, Solana `settle`, or `settle_and_seal` on a channel its payer is closing. The watchers and
  sweeps land vouchers on their own; this is the lever for planned maintenance. The voucher landed is
  the one the client edge accepted and journaled, never one the caller supplies, and landing it twice
  is a `409`.

**An open is journaled before it spends, and that is what makes it safe to retry**
([ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md) decision 8). The
node writes the channel's record — on EVM the whole `ChannelConfig`, `salt` included; on Solana the
payer-signed `open` — to `outbound-channels.log` under `state_dir` before its opening transaction is
sent, and every voucher it signs before the voucher leaves the process. So:

| the node stopped between                      | on the next attempt                                                                                                  |
| --------------------------------------------- | -------------------------------------------------------------------------------------------------------------------- |
| journaling the channel and sending its open   | a `POST /channels` toward the same receiver, for the same deposit, sends **that** channel's open, and says `resumed` |
| sending the open and seeing it confirm        | boot adopts the channel if it landed; a retry re-sends a form that can land only once                                |
| the open confirming and the journal saying so | boot finds it on chain and records that it opened                                                                    |
| signing a voucher and journaling it           | the voucher never left the node, so the watermark restored is still an honest one                                    |

"A form that can land only once": on EVM the opening deposit's authorisation is derived from the
channel's own salt, and the token spends an authorisation once; on Solana the journaled bytes are the
signed `open` itself, and a signature lands once. However often a journaled open is sent, it makes
**one** channel with **one** opening deposit. A Solana open whose blockhash expired with nothing on
chain never will land; the retry records it abandoned and opens a fresh channel. After a restart the
node knows every outbound channel it journaled and signs on each only above the highest voucher it
journaled — never below what the chain shows landed — so its watermark never goes backwards. While a
channel's open is unconfirmed it is listed `opening`, and funding or withdrawing it answers `409`.

**The `toon-channel` writes are gone.** `redeem`, `redeem-latest`, `settle`, `close` and
`cooperative-close` are deleted (#1376). **This build no longer lands a `toon-channel` claim on chain
at all**: nothing on it redeemed one but these writes (#1376 found no automatic path, though the issue
assumed one). A node draining live TOON channels does so on the last release that still has these
writes (ADR 0075, "Draining a node with live TOON channels"). **Nor does it open or fund one, on
either chain** (#1376, #1378, #1383): a `POST /channels` body without `terms` — the old
`counterparty_hex` open — answers `400` by name, and so does `/fund` on any channel that is not one
of this node's outbound x402 channels, or a body carrying the old `total`. A peering on either chain
opens and funds its own x402 channel through `POST /peers`.

### 2.3 What an operator can see

Reads are gated by a bearer token and nothing else: peers, routes (config, leased and runtime, each
labelled by source), channels, claims, node identity, the write audit log, and metrics.

`GET /channels` lists every x402 channel, inbound and outbound, each with its `direction`,
`collateral` (what still backs a new voucher, read from the chain now), `landed`, `watermark` (the
highest voucher signed, outbound, or accepted, inbound) and `status` (`opening`, `open`,
`withdrawing`, `closing`, `sealed`, or `unreadable` with a `detail`), marked `scheme:
"batch-settlement"`. Any `toon-channel` rows follow in their older shape. `GET /claims` shows vouchers
received and vouchers signed, each row with its `direction` and its `scheme`; a signed voucher's row
comes from the outbound channels' own book (`book: "outbound"`).

The audit log is the one worth knowing about: **every accepted write is retained as its own
signature**, not as a log line asserting that something happened.

`GET /dashboard` puts all of that on one page the node serves, with a form for each write that
can be made at runtime, signed in the operator's browser
([ADR 0066](../adr/0066-the-operator-dashboard-is-a-page-the-surface-serves-and-signs-in-the-browser.md)). It needs no credential to load and confers none.

### 2.4 Key rotation

Rotating the identity key **invalidates every condition already minted against the old one**. A packet
in flight, sealed to the old key, cannot be opened after the rotation and will be refused. Rotation is
therefore a scheduled action with a quiet window, not a routine hygiene task.

---

## 3. The contract

The few rules that bind.

**OP-01** `[connector]` — A connector MAY expose no operator surface at all. Configuration alone MUST
be sufficient to run one.

**OP-02** `[connector]` — A connector that exposes a write surface MUST make every write
**attributable to a specific key** and **individually revocable**. A shared secret satisfies neither:
it cannot say which operator did a thing, and losing it loses everything at once.
([ADR 0008](../adr/0008-operator-surface-splits-read-from-write.md))

**OP-03** `[connector]` — Read authority MUST NOT confer write authority. A credential that can
inspect MUST NOT thereby be able to move value.

**OP-04** `[connector]` — A connector MUST refuse to start with an operator surface enabled and no
authentication configured for either half. An unauthenticated operator surface is worse than none,
because it looks like a control plane.

**OP-05** `[connector]` — An accepted write MUST NOT be replayable.

**OP-06** `[operator]` — Announcing is the operator's verb and the **controller's** decision, never
the connector's. A connector MUST NOT push facts about itself into any network, on any schedule, ever.
([ADR 0022](../adr/0022-a-connector-answers-it-does-not-announce.md), [ADR 0046](../adr/0046-the-kind-10032-announce-is-removed-a-connector-needs-no-relay.md))

**OP-07** `[operator]` — A peering is created by an operator. A connector MUST NOT create one in
response to anything arriving over the network.
([ADR 0043](../adr/0043-purchasable-peering-is-removed.md))

---

## 4. This implementation's surface

**Non-normative.** How _this_ connector spells §3.

**Reads** — bearer token: `GET /peers` · `/routes` · `/routes/leased` · `/routes/peers` · `/channels` ·
`/claims` · `/rates` · `/identity` · `/audit-log` · `/metrics`

**Writes** — RFC 9421 HTTP Message Signature from a key on an operator allowlist, with RFC 9530
Content-Digest binding the signature to the body: `POST /packets` · `POST|DELETE /peers` ·
`POST /routes/leased` · `POST|DELETE /routes/peers` · `POST /channels` · `/channels/:id/fund` ·
`/channels/:id/withdraw` · `/channels/:id/land`

**Page** — no authentication: `GET /dashboard`, the operator dashboard, which reads and writes
through exactly the lines above from the operator's browser
([ADR 0066](../adr/0066-the-operator-dashboard-is-a-page-the-surface-serves-and-signs-in-the-browser.md)).

Two mechanisms sit behind the write half that [ADR 0008](../adr/0008-operator-surface-splits-read-from-write.md)'s
Decision does not name — **replay rejection**, and the **audit log** the retained signatures are
exposed through. Both are live and both are load-bearing for OP-05 and OP-02.

`GET /metrics` is Prometheus text exposition format; every other read is JSON.

---

## 5. Consistency

Uses exactly the vocabulary of [`CONTEXT.md`](../../CONTEXT.md) and implements
[ADR 0008](../adr/0008-operator-surface-splits-read-from-write.md),
[ADR 0022](../adr/0022-a-connector-answers-it-does-not-announce.md),
[ADR 0034](../adr/0034-a-runtime-peer-route-table-never-shadows-the-config-file.md),
[ADR 0046](../adr/0046-the-kind-10032-announce-is-removed-a-connector-needs-no-relay.md),
[ADR 0050](../adr/0050-a-connectors-url-resolves-to-its-self-description.md),
[ADR 0058](../adr/0058-a-peering-is-established-from-a-url.md),
[ADR 0059](../adr/0059-a-channel-is-derived-from-its-participants.md),
[ADR 0060](../adr/0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md),
[ADR 0066](../adr/0066-the-operator-dashboard-is-a-page-the-surface-serves-and-signs-in-the-browser.md)
and [ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md) (§2.2, amended
with its decision 11 in #1376).

**Coverage:** none of OP-01 – OP-07 is vectored and none will be. The operator surface is not a wire
surface; per [ADR 0045](../adr/0045-a-behavioural-rule-is-normative-prose-until-its-vector-lands.md)
these are prose-normative permanently and do not enter the debt ledger.

**Not yet built**, and marked rather than narrated: §2.1's boot-time deletion of a colliding runtime
row is #1076, and it is now the only one in this document.

**Since built**, and therefore described above as procedures to follow today rather than intentions:
`GET /ilp` returning the self-description (§1.4) landed in #1080, and the runtime-settable cap (§1.5)
in #1160, which put it on the write that establishes a peering. Issue #1098 is why both are called
out here — an operator reading §1.4 as a procedure and finding a not-yet-built marker in §5 lost a
round trip, and the reverse would waste one now.

The sixteen task-runbooks in `docs/operators/` remain what they are — procedures for specific boxes
and specific migrations — and several describe a fleet topology that no longer exists.
