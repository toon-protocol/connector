# ILP precedent for raw payments

> **Research note, not a decision record.** It decides nothing for
> [#1543](https://github.com/toon-protocol/connector/issues/1543) (a raw payment route: deliver a
> payer-chosen amount to a peer or a client, online or offline). It records what the Interledger
> ecosystem did when it faced the same problem, so the issue's open questions can be argued against
> precedent rather than memory. Primary sources only: the RFCs, and upstream source pinned by commit.
> Written 2026-10-23.

The question: when Interledger delivered a payer-chosen amount to a **person**, with no paid service
behind the destination, sometimes while that person was offline:

- who fulfilled the packet;
- who held the value while the person was away;
- how the person learned what had actually arrived;
- what any of it proved to a third party.

Facts (§1–§6) are kept apart from analysis (§7). Every claim in §1–§6 carries a citation. Anything
that could not be traced to a primary source is marked **unverified**.

## Sources and pins

**Vendored here.** These are read under their TOON profiles, which sit above each file's
`BEGIN VERBATIM UPSTREAM BODY` marker:

- [`docs/rfcs/0027-interledger-protocol-4`](../rfcs/0027-interledger-protocol-4/0027-interledger-protocol-4.md)
- [`0032`](../rfcs/0032-peering-clearing-settlement/0032-peering-clearing-settlement.md)
- [`0034`](../rfcs/0034-connector-requirements/0034-connector-requirements.md)
- [`0015`](../rfcs/0015-ilp-addresses/0015-ilp-addresses.md)
- [`0019`](../rfcs/0019-glossary/0019-glossary.md)
- [`0023`](../rfcs/0023-bilateral-transfer-protocol/0023-bilateral-transfer-protocol.md)

**Not vendored.** STREAM (0029), SPSP (0009), STREAM receipts (0039), payment pointers (0026) and
settlement engines (0038) are deliberately absent. [`docs/rfcs/README.md`](../rfcs/README.md) ("What
TOON does not use, and why") says this connector has no transport layer, no SPSP and no payment
pointers. They are cited here from upstream: `interledger/rfcs` at
`62abd3cc732fbb7e148f6c21c146ba9630c2edb1`. Two upstream changes matter:

- Upstream commit `5ec1e18c` (2026-03-11, "chore: deprecate STREAM receipts") moved 0039 to
  `deprecated/`. It also removed the `StreamReceipt` frame from 0029 and the receipt fields from 0009.
  Receipt text in 0029 and 0009 is therefore quoted at `8ce0feea46ba` (2024-04-30), the last version
  that had it.
- 0026 is marked deprecated in favour of `paymentpointers.org`.

**Upstream code** (branch, then commit):

| Repository                                 | Branch | Commit     |
| ------------------------------------------ | ------ | ---------- |
| `interledger/interledger-rs`               | master | `544b6dbc` |
| `interledgerjs/ilp-protocol-stream`        | master | `c62e753a` |
| `interledgerjs/ilp-plugin-mini-accounts`   | master | `32dd465c` |
| `interledgerjs/ilp-plugin-xrp-asym-server` | master | `de0f314d` |
| `interledgerjs/moneyd`                     | master | `6b221946` |
| `interledgerjs/ilp-spsp-server`            | master | `a9af4e63` |
| `interledgerjs/ilp-connector`              | master | `ef0fcb86` |
| `interledger/rafiki`                       | main   | `05833783` |
| `interledger/open-payments-specifications` | —      | `4a89dafe` |
| `WICG/webmonetization`                     | —      | `3c710521` |
| `coilhq/receipt-verifier`                  | —      | `374efb0a` |

`interledger-rs/interledger-rs` now 404s; the repository lives at `interledger/interledger-rs`.

---

## Verdict

**Two designs existed, and only one of them handled the offline recipient.**

- **The edge receives (interledgerjs: moneyd, ilp-spsp-server, ilp-protocol-stream).** The
  recipient's own process holds the STREAM server secret and fulfils. If that process is not
  connected, the parent rejects the packet. Nothing is held for it.
- **The custodian receives (interledger-rs nodes; Rafiki).** The recipient's node or account
  servicing entity (ASE) runs the STREAM receiver for every account it hosts. It fulfils on the
  account holder's behalf from its own server secret, and credits a balance it owes that holder. The
  holder can be offline throughout. Rafiki states plainly that this is a role for licensed, regulated
  custodians.

**In neither design did a fulfilment prove anything to a third party.** STREAM's fulfilment is an
HMAC over a secret both endpoints hold. What the recipient learned about the amount that arrived
always came from the custodian's own books, in one of three forms:

- a per-packet `Prepare Amount` echoed in the encrypted Fulfill;
- a balance or websocket notification;
- Open Payments' `receivedAmount`.

The one attempt at third-party evidence was RFC 0039's receipt. It was a symmetric MAC that only a
verifier sharing the secret could check, it attested only the receiver's own claim, and it has been
deprecated. Web Monetization replaced it with "go and read `receivedAmount` from the receiver's
wallet".

---

## 1. Who fulfilled the packet

### The RFC model: the receiving endpoint, whoever runs it

- RFC 0027 §Flow, Packet Lifecycle, step 9: "To accept the packet, the receiver returns an ILP Fulfill
  packet, containing the preimage of the condition".
- Prerequisite 4 says sender and receiver "agree on the destination account and packet condition"
  through a higher-level protocol "like the STREAM protocol, which establishes a shared secret"
  (`docs/rfcs/0027-…` §Prerequisites).
- STREAM puts the secret with the **server**. RFC 0029 §4.1: "A server MUST communicate the following
  values to a client … Server ILP Address … Cryptographically secure random or pseudorandom shared
  secret."
- A server may serve many connections from one secret: "a server MAY deterministically generate the
  shared secret for each connection from a single server secret and a nonce appended to the ILP
  Address … for example by using an HMAC" (ibid.).
- The fulfilment is derived from the secret, not stored. 0029 §6.2: `hmac_key =
hmac_sha256(shared_secret, "ilp_stream_fulfillment"); fulfillment = hmac_sha256(hmac_key, data);
condition = sha256(fulfillment)`. §6.3 has the receiver regenerate it from `data`.
- RFC 0009 (SPSP), "Relation to Other Protocols": "The SPSP Server generates the shared secret and ILP
  address to be used in STREAM and communicates it to the client over HTTPS."
- RFC 0026 makes a third-party host of that server an explicit goal. §Widely Usable: "it should be
  possible for a payment services provider to host payment setup protocol receiver endpoints for
  multiple entities". Its Security Considerations ask senders to verify "the entity they are sending
  to (or at least the entity hosting the payment setup protocol receiver endpoint)".

So the RFCs never require the human payee to fulfil. They require whoever holds the STREAM server
secret to fulfil, and they anticipate that this is often a wallet provider.

### interledger-rs: the node fulfils for its accounts

**Address.** An account's address is the node's address plus its username
(`crates/interledger-store/src/account.rs` L115–119: `node_ilp_address.with_suffix(details.username…)`).

**Secret.** One node-wide secret seeds every connection (`crates/interledger-stream/src/server.rs`):

- `ConnectionGenerator::new` computes `hmac_sha256(&server_secret[..], b"ilp_stream_shared_secret")`.
- `generate_address_and_secret` appends a random token to the account's address.
- `rederive_secret` recomputes the secret from that last segment.

**Fulfilment.** `StreamReceiverService` is documented as "An OutgoingService that fulfills incoming
STREAM packets. Note this does **not** maintain STREAM state, but instead fulfills all incoming
packets to collect the money". In `send_request`, when `dest.starts_with(to_address)` it calls
`receive_money(&shared_secret, …)`, returns the `fulfill`, and calls
`publish_payment_notification(PaymentNotification { to_username, from_username, amount, … })`
(same file).

**Pipeline.** In `crates/ilp-node/src/node.rs` L414–434 it sits in the node's own outgoing chain,
wrapped by `BalanceService`. A fulfil the node mints for an account therefore credits that account
(§2).

This is "the connector terminates on the user's behalf", built into the reference Rust node.

### Rafiki: the ASE's backend is the STREAM receiver

Rafiki's connector middleware runs in this order (`packages/backend/src/payment-method/ilp/connector/index.ts`
L76–101): `createStreamAddressMiddleware` → `createAccountMiddleware` → … → `createBalanceMiddleware`
→ … → `createStreamController`.

1. **Is it local?** `core/middleware/stream-address.ts` builds
   `new StreamServer({ serverSecret: config.streamSecret, serverAddress: tenantIlpAddress })` and
   decodes the destination's payment tag:
   `streamServer?.decodePaymentTag(ctx.request.prepare.destination)`.
2. **Which account?** `core/middleware/account.ts` resolves that tag to a local **incoming payment**,
   or failing that a **wallet address** ("Open Payments SPSP fallback account"). Only if neither
   matches does it look up a peer by destination.
3. **Fulfil.** `core/controllers/stream.ts` calls `streamServer.createReply(request.prepare)` and
   keeps a per-connection running total in Redis: `moneyOrReply.setTotalReceived(totalReceived);
response.reply = moneyOrReply.accept()`.
4. **Credit.** `core/middleware/balance.ts` posts a transfer into the incoming payment's (or wallet
   address's) liquidity account only `if (response.fulfill)`, and voids it otherwise.

The docs say the same: "Rafiki's `backend` service includes an Interledger connector for sending and
receiving STREAM packets", and "Every payment pointer issued by Rafiki serves as an SPSP endpoint by
default" (`packages/documentation/src/content/docs/overview/concepts/interledger.mdx`).

### interledgerjs: the user's own process fulfils

- `ilp-protocol-stream` `src/server.ts`: the `Server` holds `serverSecret`, attaches to a `plugin`,
  and is an in-process object. It fulfils only while that process runs.
- `ilp-spsp-server` README: "You must be running Moneyd to use this module". Its `index.js` serves
  `streamServer.generateAddressAndSecret()` and logs `stream.on('money', …)`.
- `moneyd` README: "It works by creating a payment channel to an Interledger connector, and then
  running `ilp-plugin-mini-accounts` locally."

The payee is the STREAM server. The parent connector only forwards.

---

## 2. Offline recipients, and custody

### interledgerjs: no holding; reject

`ilp-plugin-mini-accounts` README: "Mini-Accounts currently has no internal balance logic." A packet
for a child with no live websocket throws in `src/index.ts` `_handleOutgoingBtpPacket`:
`throw new Error('No clients connected for account ' + account)`.

That error has no `ilpErrorCode`. `ilp-connector` `src/middlewares/error-handler.ts` converts thrown
errors with `IlpPacket.errorToReject`, and `ilp-packet` defaults to `error.ilpErrorCode || 'F00'`. So
the end-to-end code is probably `F00`. That is inferred across three repositories and **not run**.

`ilp-plugin-xrp-asym-server` rejects earlier with `F02` when the account is not READY (`src/index.ts`
≈L660: `throw new Errors.UnreachableError('account must be in READY state to receive packets.')`).

Payouts to a child are paid **after** the child fulfils, and they are bound to the connection:

- `_handlePrepareResponse` on FULFILL sends "a transfer in the background to settle".
- `_sendMoneyToAccount` throws "tried to send settlement to account which is not connected".
- A failed send goes to `_increaseAmountOwed`.

So asym-server did keep an **amount owed** to a client across a failed settlement. The packet itself,
though, could only reach a connected client.

On trust, asym-server's README claims the opposite of custody: "provide ILP connection to users
without asking them to trust you with their money". `maxBalance` is "Max amount to be unsecured at any
one time". The server funds the server→client channel (`OUTGOING_CHANNEL_DEFAULT_AMOUNT`, 1 XRP, as
`outgoingChannelAmount`) and tops it up. A client must first escrow `minIncomingChannelAmount`
(10 XRP) toward the server. The moneyd README warns "don't use it with a ripple account that has too
much money" and that it "will only connect to a community network … not currently recommending it
for production". No "you are trusting the connector" wording was found in it.

### interledger-rs: a balance the node owes

The node owes the account holder the balance. Nothing requires the holder to be connected.

- **Crediting.** `crates/interledger-service-util/src/balance_service.rs` increases "the receiving
  account's balance" on fulfil. `redis/lua/process_fulfill.lua` runs
  `HINCRBY(to_account, 'balance', to_amount)`.
- **Perspective.** `get_balance` returns the balance "from the account holder's perspective". A
  positive balance is what the node owes.
- **Paying out.** Payout happens when the balance crosses `settle_threshold`, down to `settle_to`
  (same Lua), through a settlement engine.

### Rafiki: liquidity accounts, then the ASE's own ledger

Received value lands in a per-incoming-payment liquidity account. The ASE is told by webhook to
withdraw it and credit the user on its own ledger.

- `packages/backend/src/open_payments/payment/incoming/service.ts`, `handleDeactivated`: "Create
  webhook event to withdraw deactivated incoming payments' liquidity". The event carries
  `withdrawal: { accountId, assetId, amount: amountReceived }`.
- `integration/requirements/webhook-events.mdx`: "Rafiki doesn't hold _user_ account balances … any
  funds received into the incoming payment should be withdrawn and then credited to the recipient's
  account on your ledger."
- Payments to a bare wallet address use a `WEB_MONETIZATION` liquidity account and a
  `wallet_address.web_monetization` webhook with the same instruction (`account.ts`;
  `accounting.mdx`).

**Custody is explicit:**

- Rafiki is "freely available to any licensed financial service provider (FSP)", an FSP being "a
  regulated entity that provides and maintains payment accounts for its customers" (`overview/overview.mdx`;
  `overview/concepts/financial-service-provider.mdx`).
- "Rafiki should **not** be used in production environments by non-regulated entities" (ibid.).
- "Rafiki doesn't physically hold funds … actual settlement … occurs outside of both Rafiki and the
  Interledger Protocol" (`accounting.mdx`).

No document says in one sentence that "the wallet owner is offline". That follows from the code
above, in which the owner appears nowhere on the packet path.

### The RFC layer

RFC 0032 §Accounts and Balances and RFC 0038's accounting concepts define accounts strictly between
two peers. For example, 0038: "accounts payable … the amount owed by the connector to its peer for
packets its peer has fulfilled".

Neither RFC addresses holding value for an absent end party. RFC 0034 §Link Relations gives the
**child** relation and expects the parent to host IL-DCP for it, and RFC 0015 §Child Nodes addresses a
child under its parent. Neither says what happens when a child is disconnected.

---

## 3. How the payer chose the amount, and the receiver's terms

**ILPv4 only forwards.** RFC 0027 §Differences, "Forwarding, Not Delivery": senders "indicate the
minimum amount the receiver should accept for a given packet and receivers can reject packets with
less than that", and "If receivers want to receive no more than a certain amount, they can reject
packets that go too far over". Quoting is "an Application Concern" with test packets (ibid.).

**STREAM puts a floor in each packet.** 0029 §5.2, Prepare Amount: on a Prepare it is "the minimum
the receiver should accept"; on a Fulfill or Reject it is "the amount that the receiver got in the
Prepare".

- Senders "MAY send an unfulfillable test packet … to estimate the path exchange rate", and
  "SHOULD judge amounts and prices only in their own units … because destination units can be
  manipulated by the receiver" (§3.4).
- The receiver sets the ceiling with `StreamMaxMoney` (§4.4.4: "maximum amount of money they are
  willing to receive"; "MUST NOT renege on an advertisement").

**SPSP push.** The client adjusts `sendMax` and the server adjusts `receiveMax`; "If the SPSP Server
reaches their `receiveMax`, they will end the stream" (0009 §Simple push payment).

An older SPSP (commit `992de9e7`, 2018-08) let the receiver publish `balance.maximum` / `balance.current`
for invoices, with `maximum − current` used as `receiveMax`. Commit `1a367574` (2019-02) removed them.

**Per-packet limits.** These sit at the connector layer, not in STREAM:

- `F08` Amount Too Large, which SHOULD carry "the amount they received and their maximum"
  (RFC 0027 §Error Codes).
- asym-server's `maxPacketAmount` ("returns F08 if exceeded").
- ilp-connector's `balance.maximum` / `minimum`, enforced in `src/middlewares/balance.ts`. The minimum
  defaults to `-Infinity`, so by default there is no cap on what is owed to an account.

**Open Payments.** The receiver creates an incoming payment, optionally with `incomingAmount`: "The
maximum amount that should be paid into the wallet address under this incoming payment"; it is not
required (`openapi/resource-server.yaml`). If it is absent, the payer chooses the amount.

The payer quotes either fixed-send (`debitAmount`) or fixed-receive (`receiveAmount`: "The fixed
amount that would be paid into the receiving wallet address"). The quote's `expiresAt` bounds how
long the `debitAmount` is valid (ibid.).

---

## 4. How the recipient learned the amount that arrived, and what it proved

**STREAM, per packet.** The receiver echoes what arrived in the encrypted Fulfill:

- `ilp-protocol-stream` `src/connection.ts` ≈L620:
  `new Packet(sequence, IlpPacketType.Fulfill, incomingAmount, …)`.
- `interledger-rs` `receive_money` builds the reply with `prepare_amount`.

The receiving application learns the amount through `stream.on('money', amount)` and `totalReceived`
(`src/stream.ts` L390–395).

**interledger-rs, out of band.**

- `GET /accounts/:username/balance`.
- A websocket at `/accounts/:username/payments/incoming` that streams `PaymentNotification { amount,
to_username, from_username, … }` (`crates/interledger-api/src/routes/accounts.rs`).
- The notification is a Redis `PUBLISH` (`redis/mod.rs`) and is not stored. An offline holder misses
  it but keeps the balance. That reading is inferred from Redis pub/sub semantics, not stated in the
  code.

**STREAM receipts (RFC 0039, now deprecated).**

- A receipt is a "Proof provided by a STREAM receiver of the total amount received on a stream". It
  is version, Receipt Nonce, Stream ID, Total Received and an HMAC-SHA256 "using the 32 byte Receipt
  Secret, which is pre-shared between the Verifier and the Receiver".
- "The Receipt Secret MUST NOT be communicated to the Sender." The verifier credits only increases,
  and decides staleness itself (`deprecated/0039-stream-receipts/…` §Definitions, §Flow, §Overview).
- **What it proves:** the **receiver's own** attestation of a cumulative total, checkable only by a
  verifier holding the same symmetric secret. It does not name the payer. It is not publicly
  verifiable.
- Coil's verifier obtained the secret "by acting as a proxy for SPSP queries to the recipient's
  payment pointer" (`coilhq/receipt-verifier` README). Rafiki's SPSP route still honours the
  `Receipt-Nonce` / `Receipt-Secret` headers (`payment-method/ilp/spsp/routes.ts`).

**Open Payments.** `receivedAmount` is "The total amount that has been paid into the wallet address
under this incoming payment". An unauthenticated GET returns a `public-incoming-payment` that exposes
`receivedAmount` (`resource-server.yaml`, GET `/incoming-payments/{id}`). There is no receipt object
in the specification. What a third party gets is the receiver ASE's own assertion, read from the
ASE.

**Web Monetization, today.**

- `MonetizationEvent` carries `amountSent` and an `incomingPayment` URL; `receipt` and `amount` are
  deprecated.
- "All monetization receivers should be migrating from generating a STREAM Receipt to supporting
  incoming payments via Open Payments".
- The event "no longer represents an amount received; it represents an amount sent and returns a URL
  … that can be used to determine the amount received".
- A site "SHOULD track the `receivedAmount` property and ensure it is increasing"
  (`WICG/webmonetization` `specification/index.html`).

---

## 5. What a fulfilment meant

- **ILPv4.** Peers "should only consider an ILP packet executed … if the fulfillment is a valid
  preimage of the condition and … received before the Prepare's expiry" (RFC 0027 §Terminology,
  _Condition and Fulfillment_).
- **Settlement.** RFC 0034 §Settlement frames a forward as "an offer … to pay for proof-of-delivery of
  that packet".
- **STREAM.** The glossary is explicit: "In most Interledger payments, the fulfillment is known only
  to the receiver (or in the case of the STREAM protocol it is known to the sender and the receiver)"
  (`docs/rfcs/0019-glossary/…` §Fulfillment). 0029 itself never says this in one sentence; it follows
  from §6.2, where the sender computes the fulfilment in order to compute the condition.
- **What that leaves.** A STREAM fulfilment proves only that the shared-secret holder released it.
  That is the receiver's endpoint, or the sender. It is meaningful to the hops, which never hold the
  secret. It is meaningless to a third party.
- **Custodian designs.** Where an interledger-rs node or Rafiki fulfils, it proves delivery **to the
  custodian**. Whether the custodian credits the person is a matter of the custodian's books and, in
  Rafiki's case, its regulation (§2).

---

## 6. Limits, caps, expiry and unclaimed value

**Open Payments / Rafiki** (`packages/backend/src/open_payments/payment/incoming/` `service.ts`,
`model.ts`; `config/app.ts`):

- **Expiry.** It defaults to and is capped at `INCOMING_PAYMENT_EXPIRY_MAX_MS` = 2 592 000 000 ms
  (30 days). An `expiresAt` beyond it is `InvalidExpiry`.
- **States.** PENDING → PROCESSING → COMPLETED (auto when `receivedAmount ≥ incomingAmount`, or
  manual) or EXPIRED. With no `incomingAmount` the payment never auto-completes.
- **Late packets.** A non-zero packet to a COMPLETED or EXPIRED payment is refused `UnreachableError`
  (`account.ts`).
- **Partial value.** It is not forfeited. `handleExpired`: "Deactivate expired incoming payments that
  have some money. Delete expired incoming payments that have never received money". The
  `incoming_payment.expired` webhook carries the withdrawal. The docs' example: "$2.55 was received
  before the payment expired. The recipient is thus credited with $2.55" (`webhook-events.mdx`).

**Other designs:**

- **interledger-rs.** No cap on what the node may owe a receiving account was found. `min_balance` is
  checked only on the sending account (`process_prepare.lua`). The excess is paid out at
  `settle_threshold`.
- **ilp-connector.** `balance.maximum` bounds what a peer or child may owe the connector. By default
  `minimum` (what the connector may owe) is `-Infinity`.
- **asym-server.** `maxBalance` bounds unsecured exposure. It also fixes channel minimums both ways
  (§2).
- **STREAM.** Connections close after 2^31 packets (§5.1.3), and the receiver may close with
  `ConnectionClose` (§4.6). Neither is an unclaimed-value policy.

**Unclaimed value.** No primary source found specifies what happens to value a custodian owes a
person who never returns. Rafiki hands it to the ASE's ledger, where it falls under the ASE's
regulated account terms, outside Rafiki. **Unverified** beyond that.

**Coil, Xpring, Uphold, GateHub.** No primary document from these hosted wallets was found describing
receiving for offline users. **Unverified.**

---

## 7. Mapping to #1543 (analysis, not cited fact)

_Everything in this section is interpretation. Its citations point at the facts it reasons from._

**What kind of thing is the raw payment route?**

Precedent splits along exactly the line ADR 0032 draws:

- **The custodian case.** interledger-rs and Rafiki treat "my hosted account, any amount" as a
  **termination by the custodian**. It is a property of the node's own address space
  (`<node>.<user>.<token>`), resolved before peer routing (§1).
- **The edge case.** interledgerjs treats the child as a **client destination** whose process
  fulfils, and refuses when it is absent (§2).

TOON already has the second (ADR 0032; `CONTEXT.md` _Client destination_). The issue's offline case
is the first. Precedent therefore suggests not stretching ADR 0032. It suggests a distinct surface:
an address under the hub's own prefix that the hub terminates **as custodian**, crediting a debt
rather than reaching a handler.

Such a surface needs a rule against overlap with a live session at the same address. That is the
`T00` configuration error ADR 0032 already makes for app routes, otherwise the session's own
fulfilment would be shadowed. Rafiki's ordering is one precedent: local incoming payment first, then
peer. TOON's ADR 0048 precedence would decide its own.

**The hub's terms for an offline client.** The precedent is thin, and mostly says these are custodian
policy:

- **Caps.** Only per-packet (`F08`, asym `maxPacketAmount`) and per-account balance limits
  (ilp-connector `minimum`, default unbounded) existed. TOON retired accumulation limits by name
  (RFC 0032 and 0018 profiles; ADR 0033, ADR 0049). A cap on what a hub owes one client would be a
  new accumulation bound, and would need to say why it is not the retired _ceiling_.
- **Holding time and unclaimed value.** Rafiki's incoming payment expiry (30 days max) bounds
  **acceptance**, not holding. Received value is always credited (§6). Nothing upstream bounds how
  long an owed balance survives. That is left to the custodian's own terms.
- **Who funds the hub→client channel.** asym-server's server funds the outbound channel and requires
  a client deposit first (§2). ADR 0075 d7 already has the node open a connector→client channel with
  its paying half, and states that inbound collateral and payouts are independent. Lazy opening at a
  minimum owed has no upstream precedent found.

**What a fulfilment means when the hub stands in.** Precedent says exactly what the issue suspects. A
custodian's fulfilment proves delivery to the custodian, and in STREAM even that is not provable to a
third party (§5).

TOON is already in that position for every route termination (ADR 0019: "a dishonest terminating
connector can fulfil without delivering"; the defence is that "the payer chose this counterparty").
For a hub standing in for a client, the payer did **not** choose the hub as payee; it chose the
client. That is the distinction ADR 0032 was written to protect. The raw route should say so rather
than inherit ADR 0019's justification.

The voucher toward the client is a stronger artefact than anything upstream had: it is signed, it
states a running total, and it is redeemable on chain (ADR 0075 d7, d8). It proves what the hub
**committed** to the client, not what the payer sent.

**How the recipient learns the amount that arrived.** Every upstream answer came from the receiving
side's books: STREAM's echoed `Prepare Amount`, interledger-rs's notification and balance, Open
Payments' `receivedAmount`. None is a sender-side figure.

That supports the issue's direction: the figure must be the post-fee, post-conversion amount at the
termination. ADR 0040's `X-TOON-Amount` deliberately states the **price charged**, never the packet's
`amount`. Its rejected option ("Send the packet's own `amount` field") was rejected because the field
is sender-declared. At a raw termination, though, the arriving `amount` is what the last hop's
covering claim paid this node (ADR 0042), so it is no longer merely sender-declared.

For a client recipient, the latest voucher's cumulative total is the self-authenticating figure. The
recipient can compute the increment from it, much as an RFC 0039 verifier credited only receipt
increases (§4). For a third party, the recipient's own signed acknowledgement (the relay-side plan)
plays the role WM gave `receivedAmount`: an attestation by the receiving side, not a proof.

**What the packet carries beyond the amount.** Upstream's raw payments carried STREAM frames: amount
floors, stream ids, optional receipts, and connection close. No payer identity was carried; RFC 0027
notes there is no source address.

TOON has no transport layer. ADR 0018's wrap is sealed to the **terminating connector**, which at a
custodial termination is the hub, so the hub could read anything the packet carried. ADR 0032's
client-destination flow instead seals to the client's own key.

If the tip receipt travels on the relay, gift-wrapped (as decided in relay#311), precedent suggests
the packet needs little more than an amount and a correlation handle. That is analogous to Open
Payments' incoming-payment id or STREAM's payment tag in the address.

**Where TOON's records already diverge from precedent:**

- **ADR 0018/0019.** The terminating connector, not an end-to-end STREAM receiver, holds the secret
  and derives the fulfilment. That matches the custodian designs and contradicts the edge designs.
- **ADR 0069.** No condition is on the wire. So ADR 0032's sentence that a client's fulfilment is
  "verified against the packet's own execution condition" can no longer be literally true of a hop.
  This is worth reconciling in any amendment that touches client destinations.
- **ADR 0057.** STREAM's per-packet minimum is retired. There is no receiver-enforced floor, so
  "amount that arrived" cannot be inferred from what the sender asked for.
- **ADR 0075 d7 and its #1396 update.** These already contain the offline core. "A voucher still
  pending when its session dropped stays owed to its payee key" and is resent once a later session
  proves that key. That is closer to asym-server's `_increaseAmountOwed` than to anything in Rafiki,
  and it is non-custodial in the sense asym-server claimed: the debt is a redeemable voucher, not a
  ledger entry.
