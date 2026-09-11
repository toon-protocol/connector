# Architecture decision records

71 records. **Every one now carries a `**Status:**` line under its title** — that line, not this
index, is the authority for whether a record is live. This page is the map: what is live, grouped
by area; what is dead, grouped by what killed it; and what the folder still says that the code no
longer does.

> **The one-sentence model.** _The connector terminates payments the way nginx terminates SSL._
> Value travels hop to hop as a protocol the app never speaks; at the last hop the connector
> unwraps it, verifies it, and hands the app ordinary HTTP that was already paid for. That is what
> **route termination** means, why the two roles are **connector** and **app** and there is no
> third, and why the connector is a **paid reverse proxy** rather than a payment library the app
> imports. Everything in the "Payload, envelope and termination" group below is the detail of that
> sentence.

The numbers are permanent and are never reused or renumbered — they are cited over a thousand
times across this repo and from `toon-meta`, `relay` and `store`. This index groups them by
scope; it does not move them.

| If you are…                                                                 | Read                                                                           |
| --------------------------------------------------------------------------- | ------------------------------------------------------------------------------ |
| changing the connector's code or structure                                  | **[Connector architecture](#connector-architecture)**                          |
| writing or fixing another implementation (a client SDK, a second connector) | **[Protocol law](#protocol-law)**                                              |
| deploying, migrating or operating the fleet                                 | **[Fleet and operations](#fleet-and-operations)**                              |
| wondering whether something was already tried and removed                   | **[Superseded — kept for the reasoning](#superseded--kept-for-the-reasoning)** |

> **Scope note.** A record's group says _who is bound by it_, not where it is implemented.
> Protocol records are implemented in this repo but bind every implementation, which is why
> they are cited from outside it. ADR 0021 is the tiebreaker for all of them: **vectors are
> normative, prose is not.**

## The status vocabulary

Seven values, and they mean different things. Four of them describe a record that is not dead.

| Status                     | Means                                                                                             |
| -------------------------- | ------------------------------------------------------------------------------------------------- |
| **Proposed**               | **Not** live and **not** binding. Written to be argued with. Say what would make it true.         |
| **Accepted**               | Live. Binding as written.                                                                         |
| **Accepted, amended by N** | Live. A later record changed a clause without disturbing the decision. Read both.                 |
| **Accepted in part**       | Live in part. A named half is dead; the rest binds. Read the Status line for which is which.      |
| **Partly superseded by N** | Same shape, stated from the other side: a named half was replaced by record N.                    |
| **Superseded by N**        | Dead in full. Record N replaced it. Kept for the reasoning that produced it.                      |
| **Retired by N**           | Dead in full, and **nothing replaced it** — the mechanism was deleted. Kept so it is not rebuilt. |

"Retired" is the load-bearing one. A retirement record is the most valuable document in this
folder: it is the only thing standing between a future contributor and a feature this project
already built, shipped and removed on purpose.

"Proposed" is the newest and the one most easily abused. It is **not** a softer "Accepted", and it
is not the same as "Accepted, **not yet built**" — that status marks a decision that binds and has
not been implemented, and there are several. A **Proposed** record binds nothing: it may be
rejected outright, and code written against it is written on spec. It carries the same obligation
every other status does — say what would make it true — and it stops being Proposed by being argued
to a conclusion, not by being left alone long enough.

---

## Connector architecture

Internal to this codebase. Changing one of these changes how the connector is built; it does
not change what anything else must do.

| #                                                                                            | Decision                                                                     | Status                                                                                                     |
| -------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- |
| [0001](0001-rust-workspace-library-first.md)                                                 | The connector is a Rust library first, a binary second                       | Accepted                                                                                                   |
| [0002](0002-drop-mina-from-the-rust-connector.md)                                            | Settles on EVM and Solana only; Mina is dropped                              | Accepted — extended by 0065                                                                                |
| [0005](0005-claims-are-truth-balances-are-a-projection.md)                                   | Claims are the source of truth; balances are a projection                    | Accepted, amended by 0033                                                                                  |
| [0006](0006-the-connector-is-mechanism-not-policy.md)                                        | The connector is mechanism; discovery and route policy live outside it       | Accepted — restored in full by 0043                                                                        |
| [0007](0007-testing-doctrine-fakes-yes-mocks-no.md)                                          | Property tests over a pure core; fakes are allowed, mocks are not            | Accepted                                                                                                   |
| [0008](0008-operator-surface-splits-read-from-write.md)                                      | The operator surface splits read authority from write authority              | Accepted — extended by 0066                                                                                |
| [0009](0009-one-typed-config-file-no-environment-layer.md)                                   | Configuration is one typed file with no environment-variable layer           | Accepted — extended by 0034; amended by #1057                                                              |
| [0012](0012-a-signer-and-a-treasury-not-a-wallet.md)                                         | The connector holds a signer and a treasury, not a wallet                    | Accepted in part — the treasury half is gone                                                               |
| [0014](0014-metrics-surface-and-packet-correlated-logs.md)                                   | The metrics surface is decided, not accreted                                 | Accepted, amended by 0033 and 0069 (cross-hop correlation by execution condition is retired, not replaced) |
| [0015](0015-read-mostly-state-is-a-swapped-snapshot.md)                                      | Read-mostly state is a swapped snapshot; the packet path never locks         | Accepted — amended by #1069                                                                                |
| [0034](0034-a-runtime-peer-route-table-never-shadows-the-config-file.md)                     | A runtime peer/route table never shadows the config file                     | Accepted — extends 0009; survives 0043; amended by #1059                                                   |
| [0043](0043-purchasable-peering-is-removed.md)                                               | Purchasable peering is removed                                               | Accepted — **retires 0037, 0038, 0039**                                                                    |
| [0047](0047-the-configuration-schema-is-implementation-detail-capabilities-are-law.md)       | The configuration schema is implementation detail; capabilities are law      | Accepted — sharpens 0009                                                                                   |
| [0058](0058-a-peering-is-established-from-a-url.md)                                          | A peering is established from a URL; its identity is trust-on-first-use      | Accepted — **built** (#1160); completes 0034                                                               |
| [0061](0061-a-fee-attaches-to-a-peering-not-to-a-route.md)                                   | A fee attaches to a peering, not to a route                                  | Accepted — **built** (#1159); amends 0010 and 0028                                                         |
| [0062](0062-an-rfc-is-vendored-verbatim-and-profiled-never-forked.md)                        | An RFC is vendored verbatim and profiled, never forked                       | Accepted — **built** (#1173); extends 0021                                                                 |
| [0063](0063-the-ilp-packet-is-toons-dialect-not-rfc-0027s.md)                                | The ILP packet is TOON's dialect, not RFC 0027's                             | Accepted — ratifies the shipped encoding (#1174)                                                           |
| [0066](0066-the-operator-dashboard-is-a-page-the-surface-serves-and-signs-in-the-browser.md) | The operator dashboard is a page the surface serves; it signs in the browser | Accepted — **built**; extends 0008                                                                         |

---

## Protocol law

These bind **every** implementation, not just this one. A client SDK, a second connector, or a
spec written in another repo is constrained by them. This is the group most often cited from
outside this repository.

### The money model

| #                                                                                      | Decision                                                                             | Status                                                                                                  |
| -------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------- |
| [0042](0042-a-packet-carries-its-claim.md)                                             | A packet carries its claim                                                           | Accepted — **built** (#1145); supersedes 0031                                                           |
| [0004](0004-value-moves-on-fulfilment.md)                                              | Value moves on fulfilment, one claim per packet                                      | Partly superseded by 0042 — its model no longer runs anywhere (#1145); one claim per packet still binds |
| [0010](0010-flat-per-packet-fee-and-minimum-delivery.md)                               | A hop charges a flat per-packet fee; packets declare a minimum delivery              | Accepted, amended by 0042, #1072, 0061 and 0071                                                         |
| [0071](0071-a-forward-crosses-a-denomination-at-a-declared-rate.md)                    | A forward crosses a denomination at a declared rate                                  | Accepted — **built** (#1287), bar the dealing topology (#1299); amends 0010 and 0061, extends 0011      |
| [0011](0011-rejects-accumulate-fees-and-probes-discover-cost.md)                       | Rejects accumulate fees; a probe is how cost is discovered                           | Accepted, amended by 0042; extended by 0044 and 0065                                                    |
| [0051](0051-a-reject-code-binds-where-a-sender-must-act-differently.md)                | A reject code binds where a sender must act differently, and only there              | Accepted — extends 0011                                                                                 |
| [0044](0044-a-probe-answers-what-a-route-costs-and-what-it-does.md)                    | A probe answers what a route costs **and what it does**                              | Accepted — **not yet built**                                                                            |
| [0020](0020-a-price-is-flat-and-attaches-to-a-handler.md)                              | A price is flat, attaches to a handler, and buys an answer                           | Accepted, narrowed by 0040; amended by 0064 and 0065                                                    |
| [0065](0065-a-price-is-a-schedule-over-payload-length.md)                              | A price is a schedule over payload length                                            | Accepted — **built** (#984); amends 0020, extends 0011                                                  |
| [0024](0024-peer-wire-claims-sign-the-eip-712-balance-proof.md)                        | Peer claims sign the EIP-712 balance-proof digest                                    | Accepted — amended by #1136                                                                             |
| [0053](0053-a-solana-claim-binds-its-domain-the-way-an-evm-claim-does.md)              | A Solana claim binds its domain, the way an EVM claim already does                   | Accepted — **built** (#1082); the wire change has landed                                                |
| [0059](0059-a-channel-is-derived-from-its-participants.md)                             | A channel is derived from its participants, on both chains, by the same rule         | Accepted — **built** (#1158); the redeploy has not happened                                             |
| [0028](0028-a-forwarded-route-is-priced-at-the-client-edge.md)                         | A forwarded route is priced at the client edge, and carries no more than it was paid | Accepted — extended by 0029                                                                             |
| [0029](0029-a-peer-wire-arrival-to-a-priced-termination-must-cover-its-price.md)       | A peer arrival to a priced termination must cover its price                          | Accepted in part — the `F03` check stands; its ceiling cites don't                                      |
| [0033](0033-the-exposure-machinery-is-retired-not-restated.md)                         | The exposure machinery is retired, not restated                                      | Accepted — **retires `ceiling`, `flush_interval_ms`, exposure**                                         |
| [0049](0049-the-cap-bounds-one-packet-is-discovered-by-t04-and-is-set-from-outside.md) | The cap bounds one packet, is discovered by its `T04`, and is set from outside       | Accepted — **built** (#1160, the runtime cap); corrects CONTEXT.md                                      |
| [0035](0035-request-request-binding-ships-no-new-mechanism.md)                         | Request-request binding ships no new mechanism                                       | Accepted                                                                                                |
| [0052](0052-permissionless-payment-is-guaranteed-and-a-claim-is-what-authorises.md)    | Permissionless payment is guaranteed; a claim, never an identity, authorises         | Accepted — the client edge's first record                                                               |

### The wire and its carriage

| #                                                                                     | Decision                                                                    | Status                                                                                                                     |
| ------------------------------------------------------------------------------------- | --------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| [0027](0027-connectors-peer-over-btp-or-http-and-the-raw-tcp-peer-wire-is-deleted.md) | Connectors peer over BTP or ILP-over-HTTP; the raw-TCP peer wire is deleted | Accepted — supersedes 0003's and 0026's peer halves; extended by 0070                                                      |
| [0060](0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md)             | A claim proves a peering; the shared secret is deleted                      | Accepted — **built** (#1157); finished #868; vectors at schema 4                                                           |
| [0070](0070-an-onion-address-is-a-host-not-a-carriage.md)                             | An onion address is a host, not a carriage                                  | Accepted — **built** (#1273), amended in place by #1284 (`.anyone` is a host too); extends 0027; narrows 0004 in one place |
| [0021](0021-vectors-are-normative-prose-is-not.md)                                    | Vectors are normative; prose is not                                         | Accepted — **the tiebreaker for this whole group**                                                                         |
| [0045](0045-a-behavioural-rule-is-normative-prose-until-its-vector-lands.md)          | A behavioural rule is normative prose until its vector lands                | Accepted — **not yet built**; amends 0021; amended by #1052                                                                |
| [0023](0023-oer-length-determinants-are-canonical.md)                                 | OER length determinants are canonical, for every consumer                   | Accepted                                                                                                                   |
| [0003](0003-clean-room-peer-wire-versioned-client-edge.md)                            | The peer wire is redesigned freely; the client edge is versioned            | Partly superseded by 0027 — client-edge half stands; amended by #1054                                                      |
| [0026](0026-client-btp-rides-the-client-edge-peers-stay-on-the-peer-wire.md)          | Client BTP rides the client edge; peers stay on the peer wire               | Partly superseded by 0027 — one gate, two carriages stands                                                                 |

### Payload, envelope and termination

This is the group that spells out the nginx sentence at the top of this page.

| #                                                                                               | Decision                                                                      | Status                                                                                          |
| ----------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| [0018](0018-a-payload-is-sealed-to-the-terminating-connector.md)                                | A packet's payload is sealed to the terminating connector                     | Accepted — bounded by 0032                                                                      |
| [0054](0054-an-unsealed-termination-reject-answers-where-to-ask.md)                             | An unsealed termination reject says where to ask, never what the key is       | Accepted — **not yet built**; amends 0018, closes #1026                                         |
| [0019](0019-a-terminating-connector-derives-the-fulfilment.md)                                  | A terminating connector derives the fulfilment it is paid against             | Accepted — bounded by 0032; extended by 0064; `accept_if_fulfilled` rationale corrected by 0069 |
| [0025](0025-an-envelope-target-is-confined-beneath-the-handler-path.md)                         | An envelope target is confined beneath the route's handler path               | Accepted                                                                                        |
| [0064](0064-a-deadline-bounds-the-wait-for-an-app-not-the-answer.md)                            | A deadline bounds the wait for an app, not the answer it gives                | Accepted — **built** (#1183); extends 0019, amends 0020                                         |
| [0069](0069-the-execution-condition-leaves-the-wire.md)                                         | The execution condition leaves the wire                                       | Accepted — **built** (#1269); amends 0014, corrects 0019 and 0004's residue in `condition.rs`   |
| [0032](0032-a-client-destination-is-never-a-route-termination.md)                               | A client destination is never a route termination                             | Accepted — bounds 0018 and 0019                                                                 |
| [0048](0048-routing-precedence-is-length-then-rank-and-a-lease-cannot-capture-a-termination.md) | Routing precedence is length, then rank; a lease cannot capture a termination | Accepted — **partly not yet built**; bounds 0032                                                |
| [0040](0040-a-verified-payment-is-stated-to-the-app.md)                                         | A verified payment is stated to the app; an unverified one by nobody          | Accepted — supersedes 0036's conclusion                                                         |
| [0016](0016-payload-opacity-is-a-property-of-carriage.md)                                       | Payload opacity is a property of carriage                                     | Partly superseded by 0017 — first half stands                                                   |
| [0036](0036-a-paid-deliverys-attribution-stays-on-the-connector.md)                             | A paid delivery's attribution stays on the connector, never on the app        | Partly superseded by 0040 — reasoning stands                                                    |

### Discovery

| #                                                                                   | Decision                                                             | Status                                                                                |
| ----------------------------------------------------------------------------------- | -------------------------------------------------------------------- | ------------------------------------------------------------------------------------- |
| [0022](0022-a-connector-answers-it-does-not-announce.md)                            | A connector answers when asked; it still never announces             | Accepted — one consequence lost to 0027                                               |
| [0046](0046-the-kind-10032-announce-is-removed-a-connector-needs-no-relay.md)       | The kind:10032 announce is removed; a connector needs no relay       | Accepted — **built** (#1074); **retires 0030**; restores 0022, 0006; extended by 0067 |
| [0050](0050-a-connectors-url-resolves-to-its-self-description.md)                   | A connector's URL resolves to its self-description                   | Accepted — **built** (#1080); completes 0022; extended by 0067                        |
| [0067](0067-a-route-declares-its-request-shape-and-the-connector-never-reads-it.md) | A route declares its request shape, and the connector never reads it | Accepted — **built** (#1210); extends 0050, 0046                                      |

---

## Fleet and operations

Neither connector-internal nor wire law: decisions about how the fleet is run, migrated, or
how another repository is regarded.

| #                                                                                        | Decision                                                                  | Status                                                                              |
| ---------------------------------------------------------------------------------------- | ------------------------------------------------------------------------- | ----------------------------------------------------------------------------------- |
| [0017](0017-the-typescript-connector-is-a-prototype.md)                                  | The TypeScript connector is a prototype, not a reference implementation   | Accepted — a judgement about another repo                                           |
| [0041](0041-a-moving-tag-carries-the-fleets-committed-config-or-it-does-not-move.md)     | A moving tag carries the fleet's committed config, or it does not move    | Partly superseded by 0068 — Decision 3 (connector promotion) retired; 1, 2, 4 stand |
| [0056](0056-production-is-a-named-empty-tier.md)                                         | Production is a named, empty tier                                         | **Proposed** — describes an absence                                                 |
| [0057](0057-minimum-delivery-is-retired-a-claim-bounds-erosion.md)                       | Minimum delivery is retired; a claim bounds erosion                       | Accepted — **built** (#1143)                                                        |
| [0065](0065-mina-leaves-the-repository.md)                                               | Mina leaves the repository                                                | Accepted — **built** (#1205); extends 0002                                          |
| [0068](0068-a-node-repository-pins-the-connector-nothing-here-moves-a-tag-onto-a-box.md) | A node repository pins the connector; nothing here moves a tag onto a box | Accepted — **built** (#1213); partly supersedes 0041, supersedes 0055               |

---

## Superseded — kept for the reasoning

Dead in full. **None of these is deleted, and none ever will be.** A record saying "we built this
and then removed it" is the cheapest defence this project has against building it a second time.

| #                                                                                     | What it decided                                                       | What replaced it                                                                                                                                                                                                                                                 |
| ------------------------------------------------------------------------------------- | --------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [0013](0013-cut-over-through-a-parallel-address-space.md)                             | The Rust fleet runs in parallel under its own address space           | **Spent.** The migration completed (#872): the TypeScript prefix and fleet are gone. Its comparison half was superseded by 0017 first.                                                                                                                           |
| [0031](0031-a-peer-prepare-arrives-with-its-covering-claim-or-it-is-greeted.md)       | A peer PREPARE arrives with its covering claim, or it is greeted      | **[0042](0042-a-packet-carries-its-claim.md).** Every clause of its Decision was false of the shipped binary; 0042 restates the rule for every role and states the trade it makes.                                                                               |
| [0037](0037-a-purchased-peering-is-a-terminated-route-whose-work-is-a-table-write.md) | A purchased peering is a terminated route whose work is a table write | **Nothing.** [0043](0043-purchasable-peering-is-removed.md) removed purchasable peering outright. A peering is created by the operator or not at all.                                                                                                            |
| [0038](0038-a-peer-sale-lease-demotes-at-match-time-and-reaps-off-the-hot-path.md)    | A peer-sale lease demotes at match time and reaps off the hot path    | **Nothing.** [0043](0043-purchasable-peering-is-removed.md). It gave expiry to a row shape that no longer exists.                                                                                                                                                |
| [0039](0039-abuse-bounds-on-a-purchased-peering-refuse-not-refund.md)                 | Abuse bounds on a purchased peering refuse, not refund                | **Nothing.** [0043](0043-purchasable-peering-is-removed.md). Its bounds guarded a network-writable primitive; none remains. "Refuse, not refund" survives as a principle.                                                                                        |
| [0030](0030-an-operator-announces-a-node-the-node-still-does-not.md)                  | An operator announces a node; the node still does not                 | **Nothing.** [0046](0046-the-kind-10032-announce-is-removed-a-connector-needs-no-relay.md) removed the announce outright — it assumes a relay, and a pure-connector network has none. Its argument about _who_ may announce is kept.                             |
| [0055](0055-a-release-is-one-dispatch-and-the-ordering-rides-as-data.md)              | A release is one dispatch, and the deploy ordering rides as data      | **Nothing.** [0068](0068-a-node-repository-pins-the-connector-nothing-here-moves-a-tag-onto-a-box.md) retired the promotion regime this record specified before it was ever accepted or exercised — no release was cut and `:rust-release` never moved under it. |

### Records carrying superseded reasoning

Live in part, but arguing from premises a later record retired. Read the successor first, or the
reasoning will mislead you.

| Record | Read this first                                                                                                           |
| ------ | ------------------------------------------------------------------------------------------------------------------------- |
| 0003   | [0027](0027-connectors-peer-over-btp-or-http-and-the-raw-tcp-peer-wire-is-deleted.md)                                     |
| 0004   | [0042](0042-a-packet-carries-its-claim.md)                                                                                |
| 0005   | [0033](0033-the-exposure-machinery-is-retired-not-restated.md)                                                            |
| 0012   | issue #556 — the treasury half was removed with no successor record                                                       |
| 0014   | [0033](0033-the-exposure-machinery-is-retired-not-restated.md)                                                            |
| 0016   | [0017](0017-the-typescript-connector-is-a-prototype.md), [0018](0018-a-payload-is-sealed-to-the-terminating-connector.md) |
| 0026   | [0027](0027-connectors-peer-over-btp-or-http-and-the-raw-tcp-peer-wire-is-deleted.md)                                     |
| 0027   | [0033](0033-the-exposure-machinery-is-retired-not-restated.md)                                                            |
| 0029   | [0033](0033-the-exposure-machinery-is-retired-not-restated.md), [0042](0042-a-packet-carries-its-claim.md)                |
| 0036   | [0040](0040-a-verified-payment-is-stated-to-the-app.md)                                                                   |

---

## Where a record names something the tree no longer has

Checked against `crates/` and `packages/` on 2026-08-27. A record is not wrong for naming a
deleted thing — it is a record of a decision, made at a time. This table exists so a reader
does not go looking.

| Record | Names                                                                   | State in the tree                                                                                                                                                                                                                                                                                                     |
| ------ | ----------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 0001   | `connector-api`, `connector-admin`                                      | Shipped as `connector-client-edge` and `connector-operator`. `ConnectorNode` deleted (#457).                                                                                                                                                                                                                          |
| 0002   | `packages/mina-zkapp`                                                   | **Deleted** (0065), with the rest of the Mina surface. The zkApp deployed on Mina devnet is unaffected; the source is in git history.                                                                                                                                                                                 |
| 0003   | the raw-TCP peer wire, `POST /ilp/v{N}`                                 | Wire deleted (0027). The versioned edge path was never built; the edge serves `/ilp`.                                                                                                                                                                                                                                 |
| 0005   | `connector-core`, ceiling arithmetic                                    | The crate shipped as `connector-domain`. Exposure/ceiling retired (0033).                                                                                                                                                                                                                                             |
| 0007   | `connector-core`                                                        | Shipped as `connector-domain`.                                                                                                                                                                                                                                                                                        |
| 0012   | `Treasury`, `ChainClient`, `TreasuryError`                              | **Deleted** (#556). `connector-settlement`'s `SettlementBackend` does the job.                                                                                                                                                                                                                                        |
| 0013   | the parallel prefix, the TypeScript fleet                               | **Gone.** No `infra/` config carries the temporary prefix; the fleet was switched off (#872).                                                                                                                                                                                                                         |
| 0014   | `toon_exposure`                                                         | Name kept for scrape stability; **permanently zero, no producer** (0033).                                                                                                                                                                                                                                             |
| 0016   | client edge version 1, `http-proxy-handler.js`                          | **Gone.** No conformance target exists (0017).                                                                                                                                                                                                                                                                        |
| 0017   | `packages/connector`, `fleet_compare.rs`                                | **Both deleted**, as this record asked.                                                                                                                                                                                                                                                                               |
| 0023   | `packages/shared/src/encoding/oer.ts`                                   | **Package gone from this repo** (only untracked build output remains). The rule still binds.                                                                                                                                                                                                                          |
| 0024   | `SettlementChannel.sol`, `connector_domain::claim_digest`               | **Both deleted** (#578, #589) — the deletion this record required, plus a retarget it noted.                                                                                                                                                                                                                          |
| 0026   | the raw-TCP peer wire peers "stay on"                                   | Deleted (0027). Its BTP carriage and the shared codec live in `connector-btp`.                                                                                                                                                                                                                                        |
| 0027   | `flush_interval_ms`, `ceiling`, `AcceptOnlyPeerWithoutCeiling`          | Keys parsed **only to be rejected by name**; the error variant is gone (0033).                                                                                                                                                                                                                                        |
| 0029   | the `T04` exposure ceiling                                              | Retired (0033). Its own `F03` price gate is live.                                                                                                                                                                                                                                                                     |
| 0031   | `require_claim`-style enforcement on every peer PREPARE                 | Never true of the binary. Superseded (0042).                                                                                                                                                                                                                                                                          |
| 0037   | `[peer_sale]`, `deliver_peer_sale`, the peer-sale route kind            | **Deleted** (0043). `[peer_sale]` is a config key parsed only to be rejected by name.                                                                                                                                                                                                                                 |
| 0038   | the purchase lease, its demotion and reaping                            | **Deleted** (0043).                                                                                                                                                                                                                                                                                                   |
| 0039   | `max_purchased_rows`, `max_routes_per_payer`, `max_prefix_length`       | **Deleted** (0043).                                                                                                                                                                                                                                                                                                   |
| 0042   | a covering claim on forwarded arrivals                                  | **Built** (#1142), defaulting to observe per peering, so no deployed box is bound yet. The cap and the send half (`[[pay_channels]]`) are built too — the send half on **both** chains since #1146, EVM-only before it. `ClaimEnforcement::Observe` — once listed here — is **deleted** (#1077). See its Status line. |
| 0044   | a route `description`                                                   | **Not built.** No such field in `connector-config`.                                                                                                                                                                                                                                                                   |
| 0045   | the rule-classification gate, numbered rule ids                         | **Rule ids shipped** — 105 of them, `CF`/`PF`/`PM`/`ND`/`OP`, with audience tags. The **gate** is not built: no per-rule classification, no committed debt literal, no vector naming a rule id. (This row said "no rule ids exist yet" long after they did.)                                                          |
| 0030   | `connector announce`, the kind:10032 event, `[announce]`'s publish keys | **Removal pending** (0046). `[announce]`'s `addresses`/`btp_endpoint` stay — they feed the greeting.                                                                                                                                                                                                                  |
| 0009   | the `child-expander`, `apex`, `[[children]]`                            | **Removal pending** (#1057). No committed config ever used them; both become parsed-to-be-rejected keys.                                                                                                                                                                                                              |

**A removed config key is never silently dropped.** `peer_wire_addr`, `ceiling`,
`flush_interval_ms` and `[peer_sale]` are all still parsed, purely so a node whose committed TOML
still sets one stops by name at boot instead of loading with the key ignored. Finding one of these
identifiers in `crates/` is finding a tombstone, not a live mechanism.

## Retired vocabulary appearing in these records

Three terms in [`CONTEXT.md`](../../CONTEXT.md) are marked retired, and all three still appear
throughout this folder — deliberately. **These records are not rewritten into current language.**
An ADR is dated evidence; editing its words to match today's glossary would destroy the thing that
makes it worth keeping.

| Retired term                         | Retired by | Still appears in                                                                           |
| ------------------------------------ | ---------- | ------------------------------------------------------------------------------------------ |
| **peer wire**                        | 0027       | 17 record bodies, and **five filenames** (0003, 0024, 0026, 0027, 0029), which do not move |
| **exposure**, **ceiling**, **flush** | 0033       | 0004, 0005, 0014, 0027, 0029, 0031, 0033 — where the retired sense is load-bearing         |

The current words are **peer carriage** (where the bytes ride), **peer role** (the authority of an
interaction), and **cap** (a bound on one packet, never on an accumulation). All are defined in
`CONTEXT.md`.

The three terms CLAUDE.md forbids outright — **terminator**, **BLS** / Business Logic Server, and
**agent runtime** — appear in **none** of the 44 records. Nothing to migrate, and nothing to
grandfather.

---

## Related, and not an ADR

`docs/protocol/` holds the specifications the protocol records above are the decision trail for.
Since [0045](0045-a-behavioural-rule-is-normative-prose-until-its-vector-lands.md) they are **not
uniformly non-normative** — a behavioural rule is normative prose until its vector lands, per rule
rather than per document:

| file                                                               | role                                                                                                                                                                                                       |
| ------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [`configuration-spec.md`](../protocol/configuration-spec.md)       | the configuration contract — **prose-normative permanently**, since configuration is not vectorable ([0047](0047-the-configuration-schema-is-implementation-detail-capabilities-are-law.md))               |
| [`operator-spec.md`](../protocol/operator-spec.md)                 | operating a connector — **prose-normative permanently**, and deliberately thin: almost nothing here is protocol law ([0008](0008-operator-surface-splits-read-from-write.md) is scoped connector-internal) |
| [`self-description-spec.md`](../protocol/self-description-spec.md) | the node self-description — **not yet built** (#1080); a wire surface, so its rules enter [0045](0045-a-behavioural-rule-is-normative-prose-until-its-vector-lands.md)'s debt ledger                       |
| [`client-edge-spec.md`](../protocol/client-edge-spec.md)           | **the client edge specification** — corrections applied (#1073); its auth/identity/privacy surface is [0052](0052-permissionless-payment-is-guaranteed-and-a-claim-is-what-authorises.md)                  |
| [`peer-carriage-spec.md`](../protocol/peer-carriage-spec.md)       | **the peering specification** — stale citations corrected (#1073); normative for the carriage mapping, per [0045](0045-a-behavioural-rule-is-normative-prose-until-its-vector-lands.md)                    |
| [`packet-flow-spec.md`](../protocol/packet-flow-spec.md)           | routing, forwarding, termination and rejects — absorbs the frozen peer-semantics §3.1 and §4                                                                                                               |
| [`payment-spec.md`](../protocol/payment-spec.md)                   | claims, digests, fee/price/cost, settlement and the lock — replaces the frozen money-model                                                                                                                 |
| `wire-vectors.md`                                                  | the vector companion                                                                                                                                                                                       |
| `money-model-pre-868.md`                                           | **history** — the pre-#868 credit window (issue #1056)                                                                                                                                                     |
| `peer-semantics-pre-868.md`                                        | **history** — claimed normative status over retired mechanisms (issue #1065)                                                                                                                               |

[`docs/rfcs/`](../rfcs/README.md) holds the ten Interledger RFCs this connector implements or
directly profiles, vendored verbatim at a pinned upstream commit, each beneath a **TOON profile**
naming the departures and the record that governs each
([0062](0062-an-rfc-is-vendored-verbatim-and-profiled-never-forked.md)). An RFC body is the bottom
of the precedence order — vectors, then these records, then `docs/protocol/`, then a profile, then
the body — and is never edited to match this connector. That directory is CC BY-SA 4.0 rather than
the repository's MIT.

The committed vectors remain the contract for anything a vector covers.

[`CONTEXT.md`](../../CONTEXT.md) is the glossary — terms only, no decisions. These records are the
decisions. When they disagree, the record is the older document and the glossary is what the
project settled on; fix the glossary, never the record.

## Conventions

- **Every record carries a `**Status:**` line**, directly under its title and above its `**Scope:**`
  line. It names the successor by number, in both directions: a record that kills another says
  **Supersedes**/**Retires**, and the killed record names its killer. Adding a record without a
  Status line is how this folder became untrustworthy the first time.
- **Numbers are permanent.** Never renumber, never reuse, never delete a record. Supersede it
  with a new one and update both Status lines.
- **A record states its decision in its first paragraph**, before any context or options.
- **Amendments are appended in place** under an `## Update (issue #NNN)` heading rather than
  rewriting the original decision, so the trail stays readable.
- **Superseding a record means grepping for every record that cites it.** A decision is rarely
  load-bearing only where it was written: other records inherit properties from it and say so by
  name. Retire one without that sweep and its consequences stay asserted somewhere else, which is
  how a repo ends up with several documents agreeing with each other and disagreeing with the
  binary. Note which citations survive as well as which do not — most do, and saying so is what
  stops the next reader re-deriving it. ADR 0042 is the worked example: superseding ADR 0004
  disturbed 0010 and 0011, and left 0005, 0022, 0024 and 0029's actual argument untouched.
- **A record that describes a target says so**, in its Status line and its body. If the decision is
  made but unbuilt, state that plainly and list what must be true for the record to be true. A
  record written in the present tense about behaviour the binary does not have is the failure mode
  above, committed on purpose. ADR 0042 and ADR 0044 are the two that currently do this.
- **A record that claims an absence writes down what would prove it wrong, and that statement is
  run.** Directly beneath the `**Scope:**` line:

  ```
  **Falsifier:** `<path glob>` matching `<regex>` — <what a match would mean>
  ```

  It asserts that **no file matching the glob contains a line matching the regex** — the config
  field the implementation would have to add, the route it would have to register, the deleted type
  that would have to come back. `crates/connector-bin/tests/records_state_their_own_falsifier.rs`
  runs every one of them on `cargo test --workspace`, and a Status line saying "not yet built",
  "not built" or "unbuilt" **without** one fails the build. It must be **one line** — a wrapped
  marker is reported as malformed rather than silently ignored. Comment lines are skipped when
  matching, so a record may keep naming the symbol it retired. Pick a pattern the implementation
  cannot avoid rather than one that merely sounds right; where no such pattern exists, say so in
  the prose (ADR 0048 does). For a status phrase that is not a claim about this tree — a quotation
  of superseded wording, a fact about another repository — the escape hatch is the same marker,
  with the reason mandatory:

  ```
  **Falsifier:** none — <why this claim cannot be checked mechanically>
  ```

  The harness's own doc comment says what it structurally cannot catch, which is most of the
  semantic half; this convention narrows the failure mode, it does not close it.
