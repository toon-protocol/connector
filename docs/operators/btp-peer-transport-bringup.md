# Bringing up the first Rust peer link (BTP over `wss`)

> **Carriage choice.** ADR 0027 gives operators two peer carriages — BTP over `wss://` and
> ILP-over-HTTP over `https://` — selected per connector (`[peers].expose`) and per peer (the
> `endpoint` scheme). **This bring-up uses BTP**, because it is a standing high-frequency fleet link
> and because BTP is the only carriage a NAT'd operator can use, so it is the one worth proving
> first. An HTTP peering follows the same gates with the header equivalents
> (`Payment-Channel-Claim`, `Toon-Claim-Ack`, `Toon-Accumulated-Cost`) and the one difference ADR
> 0027 names: on a peering where only one side dials, the non-dialing side cannot FLUSH. Before
> [ADR 0033](../adr/0033-the-exposure-machinery-is-retired-not-restated.md) (issue #882) this also
> meant setting a lower exposure ceiling instead of relying on `flushIntervalMs`; both are retired
> along with the credit window they bounded — every peer PREPARE now carries its own covering
> claim (ADR 0031) regardless of which side dials.

Operator runbook for
[ADR 0027](../adr/0027-connectors-peer-over-btp-or-http-and-the-raw-tcp-peer-wire-is-deleted.md).

> **This replaces the four-phase migration plan that used to live at
> `btp-peer-transport-migration.md`.** That plan assumed traffic had to be drained off the raw-TCP
> peer semantics onto BTP. The 2026-08-03 audits (`toon-meta/prototypes/peer-role-audit/`) established
> that **no link has ever run on the peer semantics**: the live apex `connector-rust.toml` has no
> `[[peers]]` table, `peer-claims.log` on the Rust state volume is 0 bytes, and no peer-role
> listener is open on either box. There is nothing to drain and no dual-stack window. The raw-TCP
> transport is deleted up front; what follows is a **bring-up**, not a cutover.

## What is actually deployed today

> **Superseded 2026-08-04/05 — this section is now history.** The bring-up succeeded and the Rust
> cutover followed. Both boxes now run **only** the Rust connector at `/` (the TypeScript container
> on the store box exited at 2026-08-04T19:24:29Z), the peering is live between them, and the store
> box's edge was renamed `proxy.store.devnet` → `proxy.ario.devnet` (#774) to match the
> `g.toon.ario` prefix it serves. `proxy.store` survives only as a deprecated alias — same
> certificate, same upstream — and is slated for removal, so read every `proxy.store` URL below as
> the name that host had at the time.
>
> **Further superseded by issue #872.** The apex box this bring-up peered the store to has since
> been destroyed (toon-meta#310 / toon-meta#313) and `infra/linode-node/` deleted. There is no
> peering on this fleet at all today: both surviving boxes are client-edge-only and terminate their
> own prefix. The mechanism below (BTP carriage, the accept/dial split) is what a future peering
> would use. The one part of it that has changed since is authentication: the peering shared secret
> is deleted, and a peer proves itself with a signed claim
> ([ADR 0060](../adr/0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md)).
>
> **And by [ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md)
> (issue #1380).** A peering is now two one-way x402 channels, and a peer proves itself with a
> voucher on the one it pays on, whose signer is bound to the peering. The preconditions and gates
> below describe the `toon-channel` peering of 2026-08: a `[[peer_channels]]` row wiring
> `ClaimBook`'s channel id and counterparty key, a FLUSH (TRANSFER) when traffic stopped, and a
> stale-nonce claim. None of those exists now; read them as the record of what was proven then.

Verify before touching anything — both boxes run hand-tuned **bind-mounted** configs that lead the
repo copies.

|                      | Apex (`toon`, `proxy.devnet.toonprotocol.dev`)                                   | Store (`toon-devnet-store`, `proxy.store.devnet.toonprotocol.dev`) |
| -------------------- | -------------------------------------------------------------------------------- | ------------------------------------------------------------------ |
| TypeScript connector | yes — serves the **default** public edge at `/`, and one end of the live peering | yes — the **only** connector on the box                            |
| Rust connector       | yes, but only under `/rust/ilp` and `/rust/ilp/btp`                              | **none**                                                           |
| Inter-node link      | TS↔TS **BTP over `wss://…:443`**, live, carrying paid packets                    | same link, other end                                               |
| Rust store leg       | plain `POST https://proxy.store.devnet.toonprotocol.dev/store` — not a peering   | n/a                                                                |

Two consequences. The Rust store-box deployment is a **precondition**, not a step of this runbook
(it is tracked separately). And retiring the TypeScript connectors cannot happen until this link is
proven, because they are the default client edge and both ends of the only inter-node link.

## Preconditions

- The raw-TCP transport is deleted; `PeerTransport` and `InProcessPeerTransport` remain.
- Both carriages' peer entries/headers and role-by-auth are specified, with **shared** canonical
  vectors (ADR 0021): `payment-channel-claim` / `Payment-Channel-Claim`, `claim-ack` /
  `Toon-Claim-Ack`, `toon-accumulated-cost` / `Toon-Accumulated-Cost`. (`toon-minimum-delivery` /
  `Toon-Minimum-Delivery` was a fourth pair; it is retired with the field, ADR 0057.)
- `[peers].expose` selects the listeners; each `[[peers]]` entry has an `endpoint` URL whose scheme
  selects the dialed carriage; `[[peer_channels]]` exists and wires `ClaimBook`'s channel id,
  counterparty verification key and EIP-712 domain — and since ADR 0060 that row is the whole of
  peer authentication. A peering with no dialable intersection is a **load-time error**.
- **Peer-forwarded routes are priced and charged (#620).** Non-negotiable: a peer-forwarded route
  that is not charged is a free-write path on `g.toon`, and claims spent for free cannot be
  recharged.
- A Rust connector is deployed on the store box with its client edge behind nginx.

## Order — store accepts, apex dials

1. **Store box.** Add the peer BTP listener to the Rust connector's config and an nginx `location`
   TLS-terminating the `wss` upgrade to the Rust container, alongside the existing `/store` path
   (which is untouched). Configure the apex's `[[peers]]` entry and the `[[peer_channels]]` row
   binding its channel.
2. **Apex box.** Add `[[peers]]` (the store's `wss://` endpoint) and the matching
   `[[peer_channels]]`. Repoint **`g.toon.store` only** to peer forwarding. Leave `g.toon.ario` on
   today's HTTPS termination — that is the rollback path, and it stays warm.
3. **Soak**, then flip `g.toon.ario` the same way.
4. **Discovery**, last: point the advertised `btpEndpoint` host at the Rust listener in nginx, then
   the genesis-seed republish chain in the `toon` core repo. Nothing before this step changes what
   any external client resolves.

## Gates — in order, and do not reorder (c)

- **(a) Link up.** The BTP session establishes both directions and each side takes the `peer` role
  on its first covering claim (ADR 0060 — there is no auth frame to succeed); the session survives
  a store-container restart and reconnects without operator action.
- **(b) Routing intact.** The apex still answers prices for every route; a probe of `g.toon.store`
  returns the priced reject carrying `toon-accumulated-cost`.
- **(c) Paid write end to end with NO free-write path.** A publish is charged at the apex client
  edge, forwarded with a peer claim as `payment-channel-claim`, fulfilled, and the store-side claim
  watermark advances. A **claimless** peer PREPARE to a priced route is rejected. This is the #620
  gate. If (c) cannot be demonstrated, stop — an unmetered peer-forwarded route is worse than no
  peer link at all.
- **(d) Claim exchange complete.** A FLUSH (TRANSFER) sent when traffic stops is acknowledged with a
  `claim-ack` entry on its RESPONSE; a deliberately stale-nonce claim comes back
  `{"result":"rejected","reason":"nonce_not_advancing"}` **without** rejecting the PREPARE it rode
  on. The journaled claim verifies against the configured counterparty and is redeemable — ADR 0024's
  digest is unchanged, so the existing redemption path applies as-is.
- **(e) Discovery.** `kind:10032` announces still propagate on devnet and still resolve to a
  reachable endpoint for existing clients.

## Rollback

One config edit on the apex: point `g.toon.store` (and `g.toon.ario`, if flipped) back at
`handler_url = "https://proxy.store.devnet.toonprotocol.dev/store"` and restart. That is what
production does today, so the rollback target is the known-good current state rather than a
reconstructed one. No client-visible change, because discovery is not touched until step 4.

## Retiring the TypeScript connectors

Gated on the gates above holding for a soak window with no rollback, and tracked as its own ticket.
It is a fleet change, not a peer-transport change: the TS connectors currently serve the default
public edge on the apex and are the only connector on the store box, and the `relay` and `store`
repos still rebuild `relay-connector` / `store-connector` images **from** the TypeScript connector
image on every merge to main. Those image pipelines have to be repointed before the boxes are.

---

# The peer config surface

The configuration surface for peering, and what changed when the raw-TCP transport was
deleted (issue #677). This is the section every peer-related config error names: if a
connector refused to start and sent you here, find your error message below.

- **Decision:** [ADR 0027](../adr/0027-connectors-peer-over-btp-or-http-and-the-raw-tcp-peer-wire-is-deleted.md)
  — connectors peer over BTP or ILP-over-HTTP; the raw-TCP transport is deleted.
- **Normative detail:** [`docs/protocol/peer-carriage-spec.md`](../protocol/peer-carriage-spec.md)
  — the role rule, the two carriages, and §11's config requirements.
- **Scope of this document:** the configuration surface only (issue #677). The carriages that
  actually dial and accept are issue #676; until they land, a peering validated by this surface is
  a peering nothing traverses, and a packet routed to one is answered `T01`.

## What was removed

| Removed field                                                                                                      | Replaced by                                                                                                                |
| ------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------- |
| `peer_wire_addr`                                                                                                   | `peer_expose` — peer carriages ride this node's own listeners, not a second socket                                         |
| `[[peers]].addr`                                                                                                   | `[[peers]].endpoint` — a `wss://` or `https://` URL, not a `SocketAddr`                                                    |
| `[[peers]].credential`                                                                                             | nothing — a peering is proven by a verified voucher from the signer its `[[peer_channels]]` row binds (ADR 0060, ADR 0075) |
| `[[peer_channels]]` `channel_id`, `channel_account`, `chain_id`, `token_network`, `counterparty_key`, `program_id` | `voucher_signer` and optional `inbound_channel` — the `toon-channel` row shape is retired (ADR 0075, issue #1380)          |
| `[[pay_channels]]` `channel_id`, `channel_account`, `chain_id`, `token_network`                                    | `outbound_channel` — an x402 channel this node opened (ADR 0075, issue #1380)                                              |

All are **hard, named errors**, never a silent ignore. A `toon-channel` field is refused naming it
and ADR 0075's drain procedure: a node still holding live `toon-channel` peer channels drains them on
the last TOON-capable release first — there is no in-place migration of a channel. The devnet boxes run bind-mounted configs
that lead the repo copies, so a stale file has to stop the node rather than come up looking healthy
and never peer.

## Expose and dial are two axes

`peer_expose` says which peer carriages **this node opens a listener for**. Each peer's `endpoint`
says which carriage **this node dials that peer on**, decided solely by the URL scheme. Neither
implies the other.

```toml
peer_expose = "btp"   # "btp" | "http" | "both" | "neither"; default "neither"
```

- `wss://` selects the **BTP** carriage. Symmetric once established: after auth either side may
  originate on the one session.
- `https://` selects the **ILP-over-HTTP** carriage. Only the dialing side can originate.
- Any other scheme is a load-time error. Both are TLS-only, because a peering carries signed
  balance proofs — `ws://` and `http://` are refused too, unless the node has explicitly opted in
  (see `peer_allow_plaintext_endpoints` below).
- **No `endpoint` at all** means accept-only: this node never dials that peer, and the peer dials
  in.

### Where a peer connects: this node's own listener

There is **no peer port**. A node that exposes a peer carriage serves it on the paths its client
edge already serves, on `client_edge_addr`:

| Carriage      | Path           | What a peer sends                                             |
| ------------- | -------------- | ------------------------------------------------------------- |
| BTP           | `GET /ilp/btp` | the websocket upgrade, then `auth` on its first MESSAGE       |
| ILP-over-HTTP | `POST /ilp`    | the OER PREPARE, with its covering claim on **every** request |

So a peer's `endpoint` is `wss://<host>/ilp/btp` or `https://<host>/ilp`, and nginx needs no new
`location` beyond whatever already fronts the client edge — the `wss` upgrade included.

What tells a peer interaction from a client one on that shared socket is the **voucher on the frame**
(or, on a packet that moves no value, the peer-role challenge) and nothing else (below). The listener, the port and the bind address are explicitly **not** allowed
to decide, which is why there is nothing to open and nothing to firewall separately.

**The shared socket stays permissionless for clients.** Exposing a peer carriage adds peer handling
behind the voucher check; it puts nothing in front of anybody, and since ADR 0060 there is no peer
credential to put anywhere. A client still opens `GET /ilp/btp` presenting no credential — or, as
the deployed client does, an `auth` entry with `secret: ""`, whose contents the client edge does not
verify — and is admitted as a client; what authorizes its **writes** is the signed
payment-channel claim it puts on each frame, per
[`../protocol/client-edge-spec.md`](../protocol/client-edge-spec.md) §1.9 step 1 (_"Authorization to
write comes from the claim, never the session"_). Nothing in this runbook asks an operator to issue
a token to clients, because there is none to issue.

A peering establishes only if at least one side dials a carriage the other exposes. What the far
side exposes is not knowable from this file, so that half surfaces as an ordinary dial failure
naming the peer and the endpoint. What _is_ knowable is refused at load — see `PeerUndialable` and
`PeerRouteUndeliverable` below.

**An HTTP-only node can neither reach nor be reached by a NAT'd peer.** A NAT'd node exposes
nothing and can only dial, and it can only be reached back over a persistent session — so the
counterparty must expose BTP. If you run behind NAT, `peer_expose = "neither"` and give every peer
a `wss://` endpoint.

## A correct peering

A peering is **two one-way x402 channels**
([ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md), issue
#1380): the peer pays this node on one it opened, and this node pays the peer on one it opened. The
config names each half without opening either.

```toml
peer_expose = "btp"
state_dir = "/app/state"

# [settlement.evm] ... and its [settlement.evm.batch_settlement] sub-table:
# both channel rows below are refused by name without it.

[[peers]]
id = "store"
endpoint = "wss://store.example.net:443/btp"
# there is no `credential` here: the shared secret is deleted (ADR 0060) and a
# `[[peers]]` entry that still sets one is a named load-time error. What proves
# this peering is the `[[peer_channels]]` row below and the vouchers its
# signer signs.
# peer_answer_timeout_ms defaults to 30000
# `ceiling`/`flush_interval_ms` used to be set here -- retired along with the
# credit window they bounded (ADR 0031, ADR 0033, issue #882). Delete them
# rather than replace them; setting either now is a named load-time error.

# The inbound half: whose vouchers prove this peering.
[[peer_channels]]
peer_id = "store"
voucher_signer = "0x…"      # the peer's EVM settlement address (its channel's payerAuthorizer)
# inbound_channel = "0x…"   # optional: pin the peering to this one x402 channel id

[[routes]]
prefix = "g.example.store"
peer_id = "store"
price = 1100

# The outbound half: the channel this node pays the peer from. Required
# because a route forwards to this peering.
[[pay_channels]]
peer_id = "store"
outbound_channel = "0x…"    # an x402 channel THIS node opened with POST /channels
client_edge_url = "https://store.example.net/ilp"
```

**`voucher_signer` is the peer's settlement key, as its channel names it.** On EVM it is the peer's
settlement address — `0x` and 40 hex, the `payerAuthorizer` of the `x402BatchSettlement` channel the
peer opens toward this node. On Solana it is the peer's settlement public key in base58, that
`payment-channels` channel's `authorized_signer`. The spelling says which chain the row is on;
there is no `chain` key. It is the same key the peer's self-description publishes in
`voucherSigners`, so read it from there (`GET /ilp` on the peer) rather than from a message. The row
binds that key to the peering at boot exactly as `POST /peers` binds a published one. A key bound
twice — by two rows, or by a row and a runtime peering to a different peer — is refused.

**`inbound_channel` pins the peering.** Without it, a voucher by that signer proves the peering on
whichever channel it rides, which is what lets the peer open a new channel without a config change.
With it, only that one channel is the peer's, and the signer's vouchers on any other channel arrive
as a client's.

**`outbound_channel` must be one this node opened.** Open it first — `POST /channels` with the
peer's published `batchSettlements` terms for the chain, then `POST /channels/:id/fund` as needed
(`operator-spec.md`) — and write the channel id (EVM) or
channel account (Solana) it answers. The node reads its terms out of its own outbound-channel journal
under `state_dir` and **refuses to start** on a row naming a channel that journal does not hold. The
signing key is the chain's settlement key; there is no second one. `client_edge_url` is the peer's
own `POST /ilp`: its `POST /ilp/claim-state` is where this node restores the channel's watermark
after a lost journal.

**The two halves are two channels, never one.** A `[[pay_channels]]` `outbound_channel` that is
also a `[[peer_channels]]` `inbound_channel` is `ChannelInBothDirections`: an x402 channel moves
value one way, so this node is either its payer or its receiver.

**Both rows need the chain's `batch_settlement` sub-table** (`PeerChannelWithoutX402`,
`PayChannelWithoutX402`), per chain and no wider: that sub-table is what makes this node take part
in x402 channels on the chain at all, and without it no channel the peer opens could be admitted and
no voucher could be signed.

> _Superseded by ADR 0075 (issue #1380) — the `toon-channel` rows._ Until #1380 a `[[peer_channels]]`
> row named one `TokenNetwork` channel — `channel_id`, the `counterparty_key` a claim was verified
> against, and its EIP-712 domain `chain_id`/`token_network`, checked at boot against the registry
> (issue #1136) — or a Solana `channel_account` of TOON's own program, whose program id was
> `[settlement.solana]`'s (issue #1128, after `program_id` on the row was removed); and
> `[[pay_channels]]` named that same channel "in both roles at once". Every one of those fields is
> now refused by name (`PeerChannelToonFieldRemoved`, `PayChannelToonFieldRemoved`), pointing at
> ADR 0075's drain procedure. **If you are recovering an old config, drain the channel on the last
> TOON-capable release and delete the row — do not copy its values anywhere.**

### `credential` — deleted, and refused by name

**A peering carries no credential.** The `{peerId, secret}` shared secret is deleted — from
`[[peers]]`, from the BTP `auth` protocolData entry and from the `Toon-Peer-Auth` request header, on
both carriages together ([ADR 0060](../adr/0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md),
[`peer-carriage-spec.md`](../protocol/peer-carriage-spec.md) §1.4). A `[[peers]]` entry that still
sets `credential` stops the node at load with `PeerCredentialRemoved`, in any of its spellings —
`secret`, `secret_file` or an empty table. Delete the subtable, and delete the `*.secret` file it
pointed at; nothing on the box reads it any more.

There is **no replacement key**: not renamed, not demoted to a label, not kept as an optional
discriminator. What replaces the credential is the claim every peer frame already carried
([ADR 0042](../adr/0042-a-packet-carries-its-claim.md)) — since ADR 0075 (issue #1380), a voucher on
the channel the peer pays this node on. A signature over a voucher proves control of the key the
channel was actually opened with — strictly stronger than possession of a string both operators
wrote into their own config files, and present on every packet rather than once per session. The
rule it feeds is [Role is decided by a verified voucher](#role-is-decided-by-a-verified-voucher),
below.

Two things follow for an operator.

**Onboarding a peer no longer needs a side channel.** There is no string for the two operators to
agree out of band, which is what lets a peering be established from a public URL
([ADR 0058](../adr/0058-a-peering-is-established-from-a-url.md)) — everything the counterparty
needs is either in the self-description or derivable from it.

**The kill switch changed.** Revoking a secret used to cut a peering dead without touching a key
that also signs claims. The lever now is `DELETE /peers/:id`, an authenticated operator write on
the durable runtime peer table (ADR 0034, ADR 0058): immediate, no restart, and auditable. It
removes a **runtime** row — a peering written into this box's TOML is cut by editing that file and
restarting, because the runtime table never shadows the config file (ADR 0034).

A **receiving** node ignores a `Toon-Peer-Auth` header or a peer `auth` entry that still arrives,
rather than answering `400`. That is deliberate: it lets the two ends of a peering be upgraded in
either order without the peering going dark mid-flight while one side still sends a field the other
has stopped reading.

### The peering id is a local label

`[[peers]].id` names the peering **relation**, and since ADR 0060 nothing puts it on the wire. Role
is decided by the voucher, whose channel names its voucher signer on chain, which is bound by at most
one `[[peer_channels]]` row (`PeerChannelDuplicate` refuses a second), which names exactly one
peering — so the connector resolves the relation without any help from a name
(`peer-carriage-spec.md` §1.2).

The two operators' files therefore do **not** have to carry the same literal string. One name for
one relation is easier to read across two configs, and the peered topologies under
[`local/`](../../local/README.md) do it that way, but nothing checks it and nothing breaks if they
differ.

```toml
# apex/connector.toml                # store/connector.toml
[[peers]]                            [[peers]]
id = "apex-store"                    id = "apex-store"     # by convention, not by rule
endpoint = "wss://store…/ilp/btp"    # no endpoint: the apex dials in
```

What must agree is the **key**. Each side's `[[peer_channels]]` row names the _other_ side's
settlement key — the voucher signer on the channel the other side opens toward it — and each side's
`[[pay_channels]]` row names its _own_ outbound channel. Get a `voucher_signer` wrong and the
far side's vouchers arrive as a client's, with nothing logged; see `peer_auth_refused` below. (Until
ADR 0075 what had to agree was one shared channel, described from both points of view.)

### `peer_allow_plaintext_endpoints` — loopback and tests only

```toml
peer_allow_plaintext_endpoints = false   # the default, and the only production value
```

One top-level switch, default `false`. While it is off — which is every config that does not
mention it — `ws://` and `http://` are a hard `PeerEndpointScheme` load error, exactly as they have
always been.

Turned on, `ws://` resolves onto the **BTP** carriage and `http://` onto the **ILP-over-HTTP** one:
it widens which schemes resolve, never what they resolve to, and no other behaviour changes. It
exists so a laptop-runnable end-to-end test can point one connector at another's loopback socket
with no TLS terminator in between, and everything that sets it is of that shape: the config pairs
`crates/connector-bin/tests/two_connectors_peer.rs` and `connector-cli`'s own fixtures write at test
time, and the peered topologies under [`local/`](../../local/README.md), whose peerings dial each
other by container name over a private compose network. Every one of them is disposable by
construction, which is the only setting in which this switch is defensible.

**Never set it on a deployed node.** A peering carries every signed voucher on it — the
claims that authenticate it and move its value; in the clear, they are readable by anything on the
path. A node that does set it logs a
`WARN` naming every plaintext peering at startup, so a box that acquired one by accident says so on
every restart. There is deliberately **no per-peer form** of this switch — a per-peer field reads as
an ordinary property of that peering and gets copied into production one line at a time.

`deploy/connector-rust/connector.toml` carries the same block, commented, with every field
annotated.

### Role is decided by a verified voucher

An interaction is a peer **if and only if** it carries either of
[`peer-carriage-spec.md`](../protocol/peer-carriage-spec.md) §1.2's two proofs:

- **X1 — a voucher from a bound signer.** An x402 voucher whose signature verifies against the
  voucher signer **the chain records** for its channel, where that signer is bound to a peering — by
  a `[[peer_channels]]` row's `voucher_signer`, or by `POST /peers` from the key the peer publishes
  (and on the pinned channel only, where the row names `inbound_channel`).
- **X2 — for a packet that moves no value, the peer-role challenge.** A zero-value PREPARE carrying
  a claim-state challenge signed by such a channel's voucher signer, inside its window (no more than
  300 seconds ahead of this node's clock, and not expired).

If neither holds, for any reason, the interaction is an ordinary **client** — there is no degraded
peer and no fallthrough. Not the port, not the source address, not the carriage, not the TLS name,
no bearer string, and **not a `toon-channel` claim**, whatever channel it names and whether or not
it verifies (ADR 0075, issue #1380): only the voucher or the challenge (§1.3, and
[ADR 0060](../adr/0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md) as ADR 0075
decision 5 amends it).

Three consequences worth writing down:

- **A peering with no bound signer can never be a peering.** That is why a configured peering must
  have a `[[peer_channels]]` row (`PeerChannelUnbound`); without one, its counterparty is admitted
  as an ordinary client, and the symptom is silence.
- **Role attaches to the packet, not to the session.** A BTP session's role is whatever its current
  frame proves. A peer PREPARE carrying no covering voucher is not admitted as a peer at all — it
  gets the same 402 greeting the client edge gives (ADR 0042), so there is no unpaid peer frame left
  for anything else to take a role from.
- **A websocket that has not yet sent a packet is a client session**, which the client edge already
  serves to anyone. There is nothing extra admitted at the upgrade, and nothing to firewall.

> _Superseded by ADR 0075 (issue #1380)._ Until #1380 this section's rule was P2/P3: a claim naming a
> `channel_id` one of a peer's `[[peer_channels]]` rows configured (P2), whose signature verified
> against the `counterparty_key` that row configured (P3). `connector_peer_auth::decide_role`, which
> implemented it, is deleted.

### When a peer does not peer: `peer_auth_refused`

A connector never refuses an interaction for failing to prove a peering — refusing would tell
whoever asked which peerings this node has configured. It admits the interaction as an ordinary
client instead, silently, on the wire.

So the only place a failed peering shows up is an operator event named **`peer_auth_refused`**,
carrying the bound peer id and which requirement went unmet:

| Field    | Meaning                                                                                                                                                                      |
| -------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `peerId` | the peering the channel's voucher signer is bound to — this node's own binding, never anything the caller sent                                                               |
| `unmet`  | `P3` — a voucher or challenge on the bound channel whose signature did not recover to its voucher signer; `P3-expires` — a challenge that verified but is outside its window |

The two are deliberately **not** one bucket, because they have different fixes. `P3` is somebody
else's signature on a bound channel, or a signing fault at the payer. `P3-expires` is a right key
with a wrong clock, or a challenge signed too far ahead. (Until #1380 the buckets were `P2`, a
configured `toon-channel` the claim book had no record of, and `P3`, a claim that did not recover to
the row's `counterparty_key`.)

The event is rate-limited to one per peer id and requirement per minute, and each one reports how
many were suppressed since the last — a peering retrying every second stays one line a minute, and
still says it is still failing.

**If a peering will not establish, look for this event first.** Without it the symptom is "peering
configured, nothing peers, no error anywhere", which is exactly what happened on devnet before
role-by-authentication existed: an anonymous session was admitted as a quasi-peer and nothing
anywhere said so.

One case is deliberately silent: a voucher on a channel whose signer is **bound to no peering**.
Every ordinary client paying with a voucher presents one, so emitting there would fire
`peer_auth_refused` on essentially every client packet — both noise and a log-volume lever any
anonymous caller could pull. A `toon-channel` claim is silent for the same reason, and because it
can no longer assert a peering at all. The trap that falls out of it is worth memorising before you
debug a peering: **a peer whose voucher signer this node never bound presents as an ordinary client
with nothing logged, while a bound channel whose voucher fails to verify is loud.** If the event you
expect is missing entirely, compare this node's `voucher_signer` with the `voucherSigners` entry the
peer's self-description publishes before anything else — check `inbound_channel` too, if the row
pins one, since the peer's vouchers on any other channel are a client's — and check that this node's
`[[peer_channels]]` row exists at all, because a peering configured only on the far side is the
same silence.

### One watermark per channel

A peer's vouchers are judged by the same book a client's are, against the **channel's** one
watermark, whichever role they arrive under — so a peer bound after its channel already paid as a
client continues from where the channel stands, and nothing is counted twice
(`peer-carriage-spec.md` §1.8). What config does keep apart is direction: a `[[pay_channels]]`
`outbound_channel` that is also a `[[peer_channels]]` `inbound_channel` is `ChannelInBothDirections`.
(Until #1380 a `toon-channel` in both `[[peer_channels]]` and `[[client_channels]]` was
`ChannelInBothNamespaces`, because the two kept separate watermarks; that error is deleted.)

`[[peer_channels]]` and `[[pay_channels]]` each require `state_dir`, for the same reason
`[[client_channels]]` does: a watermark held only in memory is not a replay defence, and an outbound
channel lives in the journal there.

## The load-time errors

Every one of these stops the node before it serves anything (ADR 0009), and every message names
this document or the configuration spec.

| Error                                                                   | What it means                                                                                                           | Fix                                                                                   |
| ----------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------- |
| `PeerUndialable`                                                        | `peer_expose = "neither"` and a peer has no `endpoint` — nothing dials, nothing accepts                                 | give the peer an endpoint, or expose a carriage                                       |
| `PeerEndpointScheme`                                                    | an `endpoint` whose scheme selects no carriage (`ws://`, `http://`, `tcp://`, …)                                        | use `wss://` for BTP or `https://` for ILP-over-HTTP                                  |
| `PeerCredentialRemoved`                                                 | a `[[peers]]` entry still setting `credential` (ADR 0060, issue #1157)                                                  | delete the subtable; a voucher proves the peering now                                 |
| `PeerChannelUnbound`                                                    | a `[[peers]]` entry with no `[[peer_channels]]` row                                                                     | add a row naming the peer's `voucher_signer`, or remove the peering                   |
| `PeerChannelOrphaned`                                                   | a `[[peer_channels]]` row naming a `peer_id` no `[[peers]]` entry configures                                            | fix the `peer_id` typo, or add the peer                                               |
| `PeerChannelVoucherSignerMissing`                                       | a `[[peer_channels]]` row with no `voucher_signer` (issue #1380)                                                        | name the peer's settlement address (EVM) or key (Solana)                              |
| `PeerChannelInvalidVoucherSigner`                                       | a `voucher_signer` that is neither `0x` + 40 hex nor base58 of 32 bytes                                                 | fix the value                                                                         |
| `PeerChannelInvalidInboundChannel`                                      | an `inbound_channel` that is not an x402 channel on the `voucher_signer`'s chain                                        | fix the value, or delete the line                                                     |
| `PeerChannelDuplicate`                                                  | one `voucher_signer` or `inbound_channel` on two `[[peer_channels]]` rows — which peering it proves would be file order | one signer, one peering: delete the duplicate                                         |
| `PeerChannelWithoutX402`                                                | a `[[peer_channels]]` row on a chain with no `[settlement.<chain>.batch_settlement]` sub-table                          | add the sub-table, or peer on a chain this node takes vouchers on                     |
| `PeerChannelsWithoutStateDir`                                           | `[[peer_channels]]` with no `state_dir`                                                                                 | set `state_dir` and mount it                                                          |
| `PeerChannelToonFieldRemoved`                                           | a `[[peer_channels]]` row writing a `toon-channel` field (ADR 0075, issue #1380)                                        | drain the channel on the last TOON-capable release; rewrite the row                   |
| `PayChannelUnbound`                                                     | a route whose next hop is a peering with no `[[pay_channels]]` row (ADR 0042, issue #1145)                              | open an outbound channel with `POST /channels` and add the row                        |
| `PayChannelOrphaned`                                                    | a `[[pay_channels]]` row naming a `peer_id` no `[[peers]]` entry configures                                             | fix the `peer_id` typo, or add the peer                                               |
| `PayChannelOutboundChannelMissing` / `PayChannelInvalidOutboundChannel` | a `[[pay_channels]]` row with no `outbound_channel`, or one that is not an x402 channel id or account                   | name the channel `POST /channels` answered                                            |
| `PayChannelWithoutX402`                                                 | a `[[pay_channels]]` row on a chain with no `[settlement.<chain>.batch_settlement]` sub-table                           | add the sub-table, or pay on a chain this node settles x402 on                        |
| `PayChannelInvalidClientEdgeUrl` / `PayChannelClientEdgeUrlScheme`      | a `client_edge_url` that is not a URL, or not `https://`                                                                | the peer's own `POST /ilp` URL                                                        |
| `PayChannelDuplicatePeer` / `PayChannelDuplicate`                       | one peering on two `[[pay_channels]]` rows, or one channel paying two                                                   | one peering, one outbound channel                                                     |
| `PayChannelsWithoutStateDir`                                            | `[[pay_channels]]` with no `state_dir`                                                                                  | set `state_dir` and mount it                                                          |
| `PayChannelToonFieldRemoved`                                            | a `[[pay_channels]]` row writing a `toon-channel` field (ADR 0075, issue #1380)                                         | drain the channel on the last TOON-capable release; rewrite the row                   |
| `ChannelInBothDirections`                                               | one channel as both a `[[peer_channels]]` `inbound_channel` and a `[[pay_channels]]` `outbound_channel`                 | a peering is two channels: name each side's own                                       |
| `PeerRouteUndeliverable`                                                | a route whose next hop is a peer this node can never originate to                                                       | give the peer an endpoint, or include `btp` in `peer_expose`                          |
| `DuplicatePeerId`                                                       | two `[[peers]]` entries with the same `id`                                                                              | rename one                                                                            |
| `PeerAddrRemoved`                                                       | a `[[peers]]` entry still setting `addr`                                                                                | replace it with `endpoint`                                                            |
| `PeerWireAddrRemoved`                                                   | a config still setting `peer_wire_addr`                                                                                 | delete the line and set `peer_expose` instead                                         |
| `PeerCeilingRemoved`                                                    | a `[[peers]]` entry still setting `ceiling` (ADR 0033, issue #882)                                                      | delete the line; no replacement is needed                                             |
| `PeerFlushIntervalRemoved`                                              | a `[[peers]]` entry still setting `flush_interval_ms` (ADR 0033, issue #882)                                            | delete the line; no replacement is needed                                             |
| `PeerClaimAckTimeoutRemoved`                                            | a `[[peers]]` entry still setting `claim_ack_timeout_ms` (ADR 0075, issue #1380)                                        | delete the line; `peer_answer_timeout_ms` bounds the answer a voucher's verdict rides |
| `ClientChannelWithoutEvmSettlement`                                     | an EVM `[[client_channels]]` row on a node with no `[settlement.evm]` (issue #1138)                                     | add the table, or drop the row                                                        |
| `ClientChannelWithoutSolanaSettlement`                                  | a Solana `[[client_channels]]` row on a node with no `[settlement.solana]` (issue #1138)                                | add the table, or drop the row                                                        |
| `ClientChannelSolanaSettlementProgramIdInvalid`                         | a Solana `[[client_channels]]` row whose `[settlement.solana] program_id` is not base58 of 32 bytes                     | fix `[settlement.solana] program_id`                                                  |

A `[[pay_channels]]` row whose `outbound_channel` this node's outbound-channel journal does not
hold loads, and then stops the node at boot, naming the peer and the channel: the journal is read
only once the node starts.

`AcceptOnlyPeerWithoutCeiling` no longer exists: it required an accept-only peering to carry an
explicit `ceiling`, and both the requirement and the field are retired together (ADR 0033). The
`toon-channel` row errors — `ChannelInBothNamespaces`, `PeerChannelProgramIdRemoved`,
`PeerChannelWithoutSolanaSettlement`, `PeerChannelWithoutEvmSettlement`,
`PeerChannelSolanaSettlementProgramIdInvalid`, `PeerChannelInvalidSolanaAccount` and their
`[[pay_channels]]` twins — are deleted with the row shapes (ADR 0075, issue #1380); a file that
still writes one of those fields meets `PeerChannelToonFieldRemoved` or `PayChannelToonFieldRemoved`
instead.

Two more guard the same shape: `InvalidPeerEndpoint` (an `endpoint` that is not a URL at all — the
old `host:port` spelling lands here) and `InvalidPeerExposure` (a `peer_expose` value that is not
one of the four).

## Migrating a running box

1. Delete `peer_wire_addr`. Decide what this node should expose and write `peer_expose`.
2. Replace each `[[peers]].addr` with an `endpoint` URL, or delete it for an accept-only peering.
   Delete any `ceiling`/`flush_interval_ms` lines too — both are retired (ADR 0033) and setting
   either is now a named load-time error, not a default.
3. Delete any `credential` subtable, and the `*.secret` file it named. There is nothing to share
   with the counterparty out of band any more (ADR 0060); a `[[peers]]` entry that still sets one
   is a named load-time error.
4. If a `[[peer_channels]]` or `[[pay_channels]]` row still names a `toon-channel`, drain it on the
   last TOON-capable release first (ADR 0075); this build refuses the row by name.
5. Add a `[[peer_channels]]` row per peering naming the peer's `voucher_signer`, and — for a peering
   a route forwards to — open an outbound channel with `POST /channels` and add a `[[pay_channels]]`
   row naming it. Make sure the chain's `batch_settlement` sub-table exists and `state_dir` is set
   and mounted.

Start the node. If it refuses, the message names the field and points back here.
