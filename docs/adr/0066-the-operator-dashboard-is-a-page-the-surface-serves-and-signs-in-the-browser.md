# The operator dashboard is a page the surface serves, and it signs in the browser

**Status:** Accepted — **built** (`crates/connector-operator/src/dashboard.html`, `GET /dashboard`). Extends [0008](0008-operator-surface-splits-read-from-write.md): the front end whose buildability that record's Consequences argued for now exists, and its management half is built the one way 0008 left open — signing in the browser, with the key in the operator's hands. Consistent with [0030](0030-an-operator-announces-a-node-the-node-still-does-not.md)'s reasoning on key custody (no second process holds a key) and with [0034](0034-a-runtime-peer-route-table-never-shadows-the-config-file.md) (the page can only write runtime rows). Changes nothing in [0014](0014-metrics-surface-and-packet-correlated-logs.md): the page consumes the decided metric names and adds none. Amended by [0077](0077-the-node-keeps-a-bounded-packet-history-for-watching.md): the node keeps a bounded packet history, so "the node keeps no time series" now holds of rates and totals only.

**Scope:** connector architecture — internal to this codebase. See the [ADR index](README.md).

**Falsifier:** `crates/connector-operator/src/**/*.rs` matching `CorsLayer|ACCESS_CONTROL_ALLOW_ORIGIN|access-control-allow-origin` — the operator surface has grown a cross-origin layer, which means the dashboard has stopped being the same-origin page this record decides and a bearer token is being sent across origins.

**The connector's operator dashboard is one static HTML page, embedded in
`connector-operator` and served by the operator router at `GET /dashboard`, with no
authentication, on the socket everything else is on.** It holds nothing: every figure on
it arrives through the bearer-gated reads, and every change it makes is an RFC 9421 write
signed in the operator's browser by an operator key pasted in for the session — held in
memory, never stored, never sent. The node gains no endpoint that moves value, no key, no
cross-origin layer and no second process. The page has exactly the authority a `curl`
holding the same token and key has, and not one bit more.

## Context

An operator asking "what is my node doing?" had, until now, the procedure in
[`operator-spec.md`](../protocol/operator-spec.md) §1.4: `curl` the self-description,
`curl /metrics` if a surface is configured, and read JSON off `/peers`, `/routes`,
`/channels` and `/claims` by hand. Everything a dashboard for packet traffic, earnings,
peering and routes needs is already on the read half of the surface —
`toon_packets_total`, `toon_packets_rejected_total{code}` and `toon_fees_earned_total`
on `/metrics`; the claim journal's cumulative amounts on `/claims`, which under
[0005](0005-claims-are-truth-balances-are-a-projection.md) are the only earnings figure
that is truth rather than a counter; peerings, channels and the three route kinds each
labelled by source. Nothing needed adding to the node. What was missing was a place to
look.

Route management is different, because it is the write half. A peering, a runtime peer
route and a leased route are each created by a signed write, and a signature needs a
private key. So the real decision here is not "should there be a page" but **where the
key that signs from the page lives**, and 0008 had already said most of what matters:
the private half "lives with whoever is calling and never on the node", and a read-only
front end needs "a token and nothing else — no in-browser signing, no signing proxy".
That sentence ruled out making reads harder than they need to be; it did not decide how
a front end that also writes should sign, and this record does.

The README carried one sentence that appears to forbid this: _"A public dashboard
therefore needs a server-side holder for the token — never a token in a browser."_ It is
about a **public** status page — one strangers load — where a token in the page is a token
handed to every visitor. The operator's own browser session is not that: it is the
operator's shell with a renderer attached, and a token typed into it is in the same hands
it was in on the command line. The sentence is reworded alongside this record so the two
cannot be read as disagreeing.

## Options

1. **A separate package, separately hosted** — `packages/dashboard`, Vite and vanilla
   TypeScript like `packages/mina-usdc-faucet-web`. This is the shape that first comes to
   mind and it is wrong for this repository three times over. It would need the operator
   surface to grow a CORS layer so that a bearer token could be sent from another origin,
   which is a new property of the security surface added for a convenience. It would need
   a build, a host and a deploy of its own that must track the surface release by release,
   when the node already serves HTTP. And `packages/` is, by [`source-tree.md`](../architecture/source-tree.md),
   explicitly what is _not_ the connector, while a view of a node's own operator surface
   is nothing but the connector.

2. **A signing proxy or sidecar holding the write key on the box.** The page would stay
   read-only in the browser and hand each write to a process beside the node that signs
   it. This is the announcer of [0030](0030-an-operator-announces-a-node-the-node-still-does-not.md)
   again — a second process whose whole job is custody of a key — and it would put an
   operator write key on the box permanently, where 0008's design has the private half
   with the caller. Refused for 0030's reason.

3. **A terminal verb** — `connector dashboard`, reusing `connector send`'s signing path,
   with no dependency on browser cryptography at all. Attractive, and not what was asked
   for: a dashboard is a page at a URL the operator already has, readable on anything with
   a browser, and the binary already answers HTTP on that URL. Nothing here stops a TUI
   being added later against the same reads.

4. **A page the node serves, signing in the browser.** Chosen, and the rest of this record
   is its detail.

## Decision

- **Mounted with the surface, absent without it.** `GET /dashboard` is a route on the
  operator router, so a node with no `[operator]` answers `404`, exactly as `/metrics`
  does. An unconfigured node advertises nothing about having a dashboard.

- **The page itself needs no authentication,** because a browser cannot send an
  `Authorization` header on a navigation and because the page has nothing to protect: it
  is inert markup with no figure and no credential in it. What it is fed once loaded is
  pinned to this origin by its `Content-Security-Policy` — `default-src 'none'` and
  `connect-src 'self'` — so it loads nothing and can talk to nowhere else, and it renders
  every value it receives as DOM text, never as markup, so no field on the surface can
  become script.

- **The bearer token is kept in the tab's session storage.** It is read authority and
  nothing more (OP-03), the same standing as a cookie, and losing it on tab close is the
  right default.

- **The operator key is held in memory only.** It is imported into WebCrypto as an
  Ed25519 signing key, its public half is derived to become the `keyid`, and the hex the
  operator pasted is discarded. Nothing writes it to any storage; closing the tab forgets
  it. It is never sent: only signatures leave the page, over the same three headers
  `connector send` produces, and the node's existing verifier is the sole judge of them.
  Every write the page makes is therefore attributable, revocable, non-replayable and
  audited by mechanisms that predate the page (OP-02, OP-05) — it added none and bypasses
  none.

- **The page ships no cryptography of its own.** It uses the browser's WebCrypto Ed25519
  (Chrome 137+, Firefox 130+, Safari 17+). A browser without it gets a read-only page and
  a message saying so, not a bundled library — the page carries no dependency at all,
  which is what lets one file be the whole deliverable.

- **It offers only the writes the surface allows at runtime**: establish or remove a
  peering, write or remove a runtime peer route, lease a route. A config-file row is
  shown with its source and no button, because under
  [0034](0034-a-runtime-peer-route-table-never-shadows-the-config-file.md) and
  [0009](0009-one-typed-config-file-no-environment-layer.md) a price, a fee or a handler
  still changes by editing the file and restarting, and a page that pretended otherwise
  would be lying. Channel lifecycle — open, fund, redeem, close, settle — is shown, not
  operated: those writes spend gas and move collateral, and they stay deliberate runbook
  steps behind a `curl` until a separate decision says otherwise.

- **History is the tab's, not the node's.** The page samples `/metrics` on its poll
  interval and draws rates from the samples it has seen; a counter that falls is read as
  a restart. The node keeps no time series, as [0014](0014-metrics-surface-and-packet-correlated-logs.md)
  decided, and a Prometheus scraping the same endpoint is still how history is kept.

- **It speaks [`CONTEXT.md`](../../CONTEXT.md).** Fees are earned on fulfilment and are
  labelled fees; a terminated route shows a price; a claim's cumulative amount is what a
  counterparty has signed over. It shows "revenue" and "balance" nowhere, and it divides
  by a `decimals` the operator sets, because the node does not know which token a figure
  is in and the README's rule is that a dashboard, not the node, shows the whole-token
  figure.

## Consequences

- `crates/connector-operator`'s tests hold the page to the router: the page needs no
  token and carries the CSP; it fetches every read the router mounts and writes only to
  paths the router mounts; and the signature base it builds spells the covered components
  and algorithm from `rfc9421`'s own constants. Renaming a read, or changing what a write
  signs over, fails `cargo test` rather than an operator's browser.

- **A change to the read surface is a change to the page.** That is the cost of shipping
  a client with the server, and it is paid on purpose: the two can never be deployed out
  of step, which a separately hosted package could not promise.

- **Reachability is the box's business.** On the fleet the operator surface merges onto
  `client_edge_addr`, and each box's nginx decides what of it the internet sees — today
  `/metrics` is `404` through the relay's proxy and `401` through the store's. The page is
  reached the way `/metrics` is on that box, typically an SSH tunnel to the loopback port;
  `fleet-release-and-health.md` says where. This record puts no page on the public
  internet and no token in anything a stranger loads.

- **No health endpoint is implied.** `GET /dashboard` answering `200` says the process is
  serving and `[operator]` is configured, which `GET /ilp/identity` already said; the
  fleet health probe does not move.

- **A precision boundary is inherited, not introduced.** The surface serializes `u128`
  amounts as JSON numbers, and a browser parses those to doubles; a cumulative amount past
  2^53 base units would round on the page. At six decimals that is nine billion whole
  tokens, far beyond any channel this devnet will see, and the fix when it matters is on
  the surface (strings for amounts), not in the page.
