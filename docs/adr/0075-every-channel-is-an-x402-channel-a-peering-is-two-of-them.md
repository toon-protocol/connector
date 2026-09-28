# Every channel is an x402 channel; a peering is two of them

**Status:** Accepted (owner decision, 2026-09-27, issue #1371) — **mostly not yet built**. The implementing steps are #1371's sub-issues, and the binary still speaks `toon-channel` claims until they land. Built so far: decision 2's paying half of the settlement port -- `BatchSettlementPayer` and its contract suite (#1373), implemented on EVM (#1374) and Solana (#1375) and on the fake, each held to the suite unmodified. Decision 11's operator surface (#1376): `POST /channels` opens an outbound x402 channel, `/fund` tops it up, `/withdraw` starts and then finishes a withdrawal (EVM) or close (Solana), `/land` lands the held latest voucher on an inbound channel, `GET /channels` and `GET /claims` show each channel's direction, and `redeem`, `redeem-latest`, `settle`, `close` and `cooperative-close` are deleted; the EVM `toon-channel` branch of `/channels` and `/fund` is deleted with them, and the Solana one stays until #1383 because `local/keys.sh` still opens its channels through it. With it, decision 8's outbound-channel journal entry: an outbound channel's record is journaled before its opening transaction is sent, and every voucher signed on it before the voucher is handed out, so a retried open resumes the journaled channel and a restart restores it at its signed watermark (the port's `prepare_open`, `open_prepared` and `restore_outbound`, which the suite holds to it). Nothing on the packet path signs a voucher yet: `[[pay_channels]]`, the peering and payouts are #1378, #1379 and #1380. Also decision 5's role rule (#1377) — a voucher, or a peer-role challenge, from a channel whose voucher signer is bound to a peering decides `peer` on both carriages, and binding a signer is a runtime operation; its two sources (#1378, #1380) are not built, so no node binds one yet. The owner decided the points this record first left open before accepting it (see "Decided by the owner before acceptance"), and its facts about the `payment-channels` deployment were read on the day of acceptance. It **supersedes in part** [0074](0074-a-client-may-pay-over-an-x402-batch-settlement-channel.md): decision 1 (scope), decision 9's receive-only port, and the clauses of decisions 2, 3, 4, 5, 6 and 8 that rest on decision 1 (listed under "The sweep"). It **retires** [0024](0024-peer-wire-claims-sign-the-eip-712-balance-proof.md) and [0053](0053-a-solana-claim-binds-its-domain-the-way-an-evm-claim-does.md) (the `toon-channel` claim schemes), [0059](0059-a-channel-is-derived-from-its-participants.md) (the derivation rule), and [0026](0026-client-btp-rides-the-client-edge-peers-stay-on-the-peer-wire.md)'s payout netting (#700). It **amends** [0042](0042-a-packet-carries-its-claim.md) (a peering pays and is paid over two channels), [0058](0058-a-peering-is-established-from-a-url.md) (`POST /peers` opens the outbound channel only), [0060](0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md) (a peering is proven by a voucher, or by a claim-state challenge, from a bound channel's voucher signer), [0005](0005-claims-are-truth-balances-are-a-projection.md) (the nonce watermark goes; the journal holds an outbound channel's config) and [0050](0050-a-connectors-url-resolves-to-its-self-description.md) (the self-description drops `settlements`). It disturbs [0021](0021-vectors-are-normative-prose-is-not.md): `schema_version` goes to **7**. It leaves [0010](0010-flat-per-packet-fee-and-minimum-delivery.md) and [0061](0061-a-fee-attaches-to-a-peering-not-to-a-route.md) untouched. Those records' Status lines, the index and `CONTEXT.md` were updated on acceptance; `docs/protocol/` is amended with each implementing step (step 12). The one release prerequisite, an ERC-3009 devnet USDC, was already met by #1337 (see "Prerequisites").

**Scope:** protocol law. It binds every implementation, because it removes a claim scheme from the wire, changes what proves the peer role, and changes the greeting and the self-description. The port shape (decision 2), the operator surface (decision 11) and the config (decision 9) are connector architecture. See the [ADR index](README.md).

**Falsifier:** `crates/connector-runtime/src/peering.rs` matching `OutboundChannels|BatchSettlementPayer` — `POST /peers` does not yet open its outbound channel on the paying half: it still derives and opens a `toon-channel` (decision 4, #1378). The peering can reach the paying half only through the journaled `OutboundChannels` or the port itself, so a match means step 4 has landed and this record's Status line must say so.

**Falsifier:** `crates/connector-vectors/src/**/*.rs` matching `SCHEMA_VERSION: u32 = 7` — the wire still carries `toon-channel` claims at `schema_version` 6. Decision 14 moves it to 7 and nothing else does, so a match means step 9 has landed and the Status line must say the wire is built.

**A connector settles only on x402 `batch-settlement` channels, on both chains.** On EVM every
channel is an `x402BatchSettlement` channel at `0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003`; on
Solana every channel is a `payment-channels` channel under
`CHNLxYvVA28MJP9PrFuDXccuoGXAx7jBacfLEkahyGsX`. Both are constants of the binary. TOON's
`TokenNetwork`, `TokenNetworkRegistry`, `RollingSwapChannel` and Solana payment-channel program are
retired from the connector. **Such a channel moves value one way, so a peering is two of them:** A→B
carries A's vouchers to B, B→A carries B's to A, and each node opens and funds only its own outbound
channel. A connector paying a client back does so over a connector→client channel it opens the same
way. **Every claim is a voucher**: one claim scheme, one freshness rule (0074's amount-only
watermark), one greeting entry type, and one settlement port with a paying half and a receiving
half. Routing fees are untouched.

## What 0074 decided, and the one premise this record removes

[0074](0074-a-client-may-pay-over-an-x402-batch-settlement-channel.md) admitted x402 channels at the
client edge only. It weighed "x402 channels as TOON's settlement layer everywhere" and rejected it in
one line: _"bidirectional peering and client payout have no x402 shape."_ Its table named four TOON
rules an x402 channel runs into. This record goes back through the same four:

| TOON rule, as 0074 stated it                            | At the client edge (0074)      | Everywhere (this record)                                                                                                                                                                                     |
| ------------------------------------------------------- | ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| A peering pays and is paid on one channel (0042, #1146) | No conflict: client→connector  | **Not a rule of 0042.** 0042 requires that a packet carry its claim, not that both directions share a channel. Two one-way channels satisfy it (decision 4).                                                 |
| One live channel per pair, derived (0059)               | Reconcilable (0074 decision 2) | **The reason for it goes away.** 0059 derives the id so that two nodes land on the _same_ channel without exchanging an id. Under two channels nothing is shared: each node names only its own (decision 4). |
| A nonce may advance at an unchanged amount (0005)       | Resolved (0074 decision 3)     | **Resolved the same way,** with the one job a nonce did that an amount cannot (a zero-value packet on a peering) done by a signed challenge instead (decision 5).                                            |
| Payout nets against the client's deposit (0026, #700)   | The loss accepted              | **The loss accepted, for every client and every peering.** Stated, not hidden (Consequences).                                                                                                                |

The rejection in 0074 rested on the first row being a requirement. It was a property of
`TokenNetwork`'s two-sided channel, which the peering code grew around: `[[peer_channels]]` and
`[[pay_channels]]` naming one channel "in both roles at once" is the deployed shape
(`connector-config/src/pay_channel.rs`, 0058's Update for #1217), not something any record requires.
Nothing else in 0074's reasoning is disturbed. Its admission rules, its watermark, its safety analysis
and its sponsor rule are exactly what this record extends to every channel.

## What two kinds of channel cost

- **Two sets of on-chain code to deploy, record, audit and keep in step with the binary.** TOON's
  contracts and program are this project's to deploy on every network. Both have already forced a
  breaking redeploy once each: 0059's `channelEpoch` (a new `TokenNetwork` on 2026-08-28, stranding
  every channel on the old one) and 0053's domain-bound message. The x402 contract is ownerless,
  immutable, audited and deployed at one address on every network; `payment-channels` is audited and
  deployed at one program id on every cluster.
- **Two claim schemes, two freshness rules, two greeting entries and two sets of vectors**, which
  every client and every peer has to implement both of.
- **A client that needs native gas.** A `toon-channel` payer must hold ETH or SOL before it can pay
  anyone. On x402 it needs only the token (0074, Consequences).

## Decision

### 1. One channel type, fixed by the binary

- **EVM:** `x402BatchSettlement` at `0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003`, with its deposit
  collectors at their canonical addresses (0074, Sources). **Solana:** `payment-channels` at
  `CHNLxYvVA28MJP9PrFuDXccuoGXAx7jBacfLEkahyGsX`, SPL Token mints only (Token-2022 stays refused).
- **Both are constants of the binary, never config.** A voucher's EIP-712 domain names the contract's
  address and a Solana voucher binds a PDA of the program, so a configurable address is only a way to
  point a node at code whose claims it cannot redeem (0074 decision 4, as amended by the #1349 review).
- **Boot refuses a chain the code is absent from.** A node reads the contract's code (EVM) or the
  program account (Solana) at its configured `rpc_url` before serving, and refuses to start, by name,
  when it is absent. A network x402 has not deployed to is unsupported, loudly.

### 2. One settlement port, with a paying half and a receiving half

`SettlementBackend` and its three implementations (`EvmSettlementBackend` over `TokenNetwork`,
`SolanaSettlementBackend` over TOON's program, `InMemorySettlementBackend`) are **deleted**, with
their contract suite. `BatchSettlementBackend` (`connector-settlement/src/batch/port.rs`) grows into
the single port. 0074 decision 9 declined to bend `SettlementBackend` to fit a one-way channel; the
same reasoning now deletes it rather than bending it, because nothing is left on the other side of it.

- **Receiving half (exists):** admit a presented channel, restore a journaled channel, read channel
  state, land a voucher.
- **Paying half (new):**
  - open and deposit toward a counterparty receiver, under the receiver's published terms;
  - top up by an increment;
  - sign a voucher for a cumulative amount;
  - start and finish a withdrawal: EVM `initiateWithdraw` then `finalizeWithdraw`; Solana
    `request_close`, then `distribute` and `reclaim` once the channel is sealed or its grace period
    has run;
  - read its own outbound channel's state.

**One implementation of both halves per chain**, as a module in the existing crate, reusing that
crate's single `RpcTransport` ([0073](0073-settlement-rpc-may-ride-the-circuit-once-every-wait-on-it-is-bounded.md))
and the settlement key's one nonce sequence. **The in-memory fake implements both halves**, and is the
fake every higher-level test runs over ([0007](0007-testing-doctrine-fakes-yes-mocks-no.md)). The
contract suite (`batch::contract::assert_upholds_the_contract`) grows to cover the paying half and runs
against the fake, anvil with the x402 bytecode placed, and `solana-test-validator` with `CHNLx…` in
genesis.

### 3. Opening and signing, per chain

**EVM, paying side.** The node builds a `ChannelConfig`:

- `payer` is its EVM settlement address;
- `payerAuthorizer` is **the same settlement address** — `payerAuthorizer == payer`, which the
  contract permits and 0074 decision 2's nonzero rule is satisfied by;
- `receiver` and `receiverAuthorizer` are both the counterparty's published settlement address, as
  0074 decision 2 requires of any channel the counterparty will admit;
- `token` is the token the two share;
- `withdrawDelay` is at least the counterparty's published minimum;
- `salt` is fresh.

It deposits by sending `deposit(config, amount, collector, collectorData)` itself and paying its own
gas; no facilitator is involved. The deposit still passes through one of x402's collectors, because
the contract has no other way in: `ERC3009DepositCollector` where the token has ERC-3009 (Circle
USDC, and the local stack's FiatToken), with a fresh collector salt per deposit because it makes the
ERC-3009 nonce; `Permit2DepositCollector` after a one-time `approve` where it does not (a
node holds ETH, so a one-time `approve` costs it nothing that matters). A top-up is another `deposit` into the same config. Vouchers are
EIP-712 `Voucher(channelId, maxClaimableAmount)` under the `x402BatchSettlement` domain, and the first
voucher on a new channel carries `channelConfig`, exactly as a client's does today.

**Which key signs a voucher: the chain's settlement key, on both chains** (owner's decision,
2026-09-27; #1371 had proposed the `[signer]` address for EVM's `payerAuthorizer`). On EVM
`payerAuthorizer == payer`; on Solana `authorized_signer` is the payer. The reasons:

- **One rule on both chains.** Solana has to use the ed25519 settlement key anyway, because `[signer]`
  is secp256k1 and cannot be an `authorized_signer`.
- **It is today's rule.** An EVM peer claim is signed by the settlement key now, not by `[signer]`
  (`connector-cli/src/runtime.rs`, `peer_claim_identity`), and a Solana one by the Solana settlement
  key (`peer_claim_identity_solana`).
- **`[signer]` stays the node's identity** — gift wrap ([0018](0018-a-payload-is-sealed-to-the-terminating-connector.md))
  and the self-description — and never gains spending authority. [0058](0058-a-peering-is-established-from-a-url.md)'s
  three identities stay apart.
- **Separating the keys would buy little.** Both are hot on the same node, and the settlement key
  already signs every deposit, top-up and withdrawal on the same channel.

**Solana, paying side.** The node builds a `payment-channels` `open` in which:

- it is itself `payer`;
- the counterparty's sponsor key is fee payer, `rent_payer` and `payee`;
- `authorized_signer` is this node's Solana settlement key;
- the one distribution entry is the counterparty's receiving account at 10000 bps;
- `grace_period` is at least the counterparty's published minimum.

It posts the payer-signed `open` to the counterparty's `sponsorEndpoint` (0074 decision 9). That is
the only way a receiver keeps the `payee` seat, which 0074 decision 5 shows it must hold if it is to
land its latest voucher after the payer asks to close. The counterparty's `min_sponsored_deposit`
bounds the opening deposit. The node tops up with `top_up` and signs the 50-byte voucher message with
`expires_at = 0`.

**Receiving side, both chains: unchanged** from 0074 decisions 2, 3 and 5 — the admission rules, the
amount-only watermark, byte-identical retransmission, the `WithdrawInitiated` watcher, the Closing
watcher with `settle_and_seal`, and the sweeps. A peer's channel is admitted by exactly the rules a
client's is.

### 4. A peering is two channels

This **amends [0042](0042-a-packet-carries-its-claim.md)**: a peering pays over one channel and is
paid over another. **It amends [0058](0058-a-peering-is-established-from-a-url.md)**: `POST /peers`
opens and funds only this node's outbound channel. **It retires
[0059](0059-a-channel-is-derived-from-its-participants.md).**

- **`POST /peers` still reads the peer's self-description** ([0050](0050-a-connectors-url-resolves-to-its-self-description.md)),
  still checks that the two nodes share a chain and a token, and still writes a durable runtime
  peering. What it opens is this node's channel toward the peer, and nothing else. Trust-on-first-use
  is unchanged and unstrengthened (0058).
- **"Is there a live channel with this peer?" becomes a lookup, not a derivation**: this node's own
  outbound channels whose receiver is that peer's settlement address. Several live channels to one peer
  are legal, as they already are at the client edge.
- **The inbound half is admitted, not configured.** The peer's voucher on its own channel is admitted
  by the receiving half's rules. The channel is then **bound** to that peer when its voucher signer —
  EVM `payerAuthorizer`, Solana `authorized_signer` — is the peer's settlement address or key on that
  chain as its self-description publishes it (decision 10), or the key a config row names. On EVM the channel shows up with its first voucher; on Solana it shows
  up when the peer posts its `open` to this node's sponsor endpoint. Neither operator pastes the other's
  channel id anywhere.
- **Symmetry survives without derivation.** 0059's argument was that _"B must be able to add A the same
  way A added B and land on the same channel, without either telling the other an id."_ Under this
  record neither lands on a shared channel, because there is none. Each writes `POST /peers` naming the
  other's URL, each opens its own outbound channel, and each binds the other's inbound channel by the
  published key. The rejected option 0059 argued against — _"which of my several channels with this
  counterparty should this peering use?"_ — has an answer here: a node chooses among its own outbound
  channels, and the other side never has to agree.
- **Removing a peering** stops signing on the outbound channel and leaves it ready to withdraw
  (decision 11). The inbound channel stays admitted until the peer closes it, and the watchers land its
  latest voucher as they would a client's.
- **Dealing** ([0071](0071-a-forward-crosses-a-denomination-at-a-declared-rate.md)) is unchanged: each
  peering's token is read from its channels, and conversion at the declared rate still happens between
  them.
- **Carriage is unchanged.** BTP and ILP-over-HTTP, onion endpoints included
  ([0027](0027-connectors-peer-over-btp-or-http-and-the-raw-tcp-peer-wire-is-deleted.md),
  [0070](0070-an-onion-address-is-a-host-not-a-carriage.md)), carry a voucher where they carried a
  claim.

### 5. The peer role: a voucher, or a challenge, from a bound channel's voucher signer

This **amends [0060](0060-a-claim-proves-a-peering-and-the-shared-secret-is-deleted.md)**. An
interaction has role `peer` if and only if it carries either:

- **a voucher on a channel bound to that peer** (decision 4), verified against the signer the chain
  records for that channel; or
- **for a packet that moves no value, the voucher claim-state challenge** (#1364's struct and message:
  EVM `ClaimStateChallenge(bytes32 channelId,uint256 expires)` under the `x402BatchSettlement` domain;
  Solana Ed25519 over `"toon-voucher-claim-state-challenge-v1" ‖ channelAccount ‖ expires`), signed by
  a bound channel's voucher signer and refused once `expires` has passed.

0060's substance is kept: a peering is proven by a signature, never by a shared secret, and the
evidence names its own peering. What changes is which signature. It replaces the `toon-channel` claim
as the role proof on both carriages, and the client BTP `auth_channel_proof` declaration, which signs
under the `TokenNetwork` domain today. **This supersedes 0074 decision 1's "a voucher is never a peer
claim" and decision 6's "a voucher never decides the peer role."**

**Zero-value peer packets carry no voucher** — 0074 decision 3's rule, extended from the client edge
to the peer wire — and carry the challenge instead when they need a role. A challenge names a channel,
and a channel names its receiver, so a challenge is meaningful only to the node it is addressed to;
within `expires` it is nonetheless a bearer proof for zero-value traffic, and `expires` is to be kept
short (see Consequences).

The same message now serves two purposes: asking a receiver where a channel's watermark stands, and
proving the peer role. Both say the same thing — _I control this channel's voucher signer, until
`expires`_ — to the same party, and neither moves value, so one message is not a confusion between
them. What must stay separate is the challenge and the voucher, and `connector-signer`'s separation
tests (a challenge is never a voucher, in both directions) cover the peer-role use too.

### 6. Outbound claims, the watermark, and the ack

- **`[[pay_channels]]` and `OutboundClientLedger` sign vouchers** on this node's outbound channel to
  the next hop. 0042's rule is unchanged: every forwarded PREPARE is covered, now by a voucher, for
  `amount_after_fee(amount, fee)`.
- **The next hop's `POST /ilp/claim-state` (`scheme: "batch-settlement"`) is the watermark authority on
  restore**, as the `toon-channel` claim-state is today. After a restart or a lost journal, a node never
  signs a voucher that fails to advance. The chain's `totalClaimed` / `settled` is a lower bound only,
  since it trails the receiver's watermark until the receiver lands.
- **The ack** (`Toon-Claim-Ack`, and the BTP `claim-ack` entry) carries its verdict on a voucher,
  unchanged in shape. Its vector cases are regenerated for vouchers.

### 7. Client payouts: a connector→client channel, and no netting

This **retires [0026](0026-client-btp-rides-the-client-edge-peers-stay-on-the-peer-wire.md)'s payout
netting** (its #700 update).

- **`ClientPayoutLedger` signs vouchers on a connector→client x402 channel** that this node opens with
  its paying half, toward the client's published or declared address. It supersedes 0074 decision 1's
  "a client that expects a payout must hold a `TokenNetwork` or TOON-program channel."
- **The headroom formula `deposit − owed + credited` is deleted.** A client's inbound collateral and
  its payout channel are independent: value paid out does not raise what the client may spend, and
  value the client pays in does not fund its payouts.
- **Delivery is unchanged**: a BTP TRANSFER over the client's session carries the payout voucher, where
  it carries a payout claim today (0026's #770 update), and `record_payout_once`'s dedupe is unchanged.

### 8. Claims, the domain and the journal

This **retires [0024](0024-peer-wire-claims-sign-the-eip-712-balance-proof.md) and
[0053](0053-a-solana-claim-binds-its-domain-the-way-an-evm-claim-does.md)** — the EIP-712
`BalanceProof` claim and the 96-byte `TOON-BALPROOF-V2` message — and **amends
[0005](0005-claims-are-truth-balances-are-a-projection.md)**. Their principle survives in the scheme
that replaces them: a claim's signature binds its domain, which 0074 decision 4 already requires of a
voucher on both chains.

- **`ClientClaim` loses its `Evm` and `Solana` (`toon-channel`) variants, and `scheme` becomes
  required.** An absent `scheme`, or `"toon-channel"`, is refused structurally, by name, the way
  `blockchain: "mina"` is refused today. This supersedes 0074 decision 4's "when it is absent, the claim
  is `toon-channel`."
- **`connector-domain`'s nonce rules are deleted**: `Watermark { nonce, … }`, `validate_claim` and
  `advance_watermark`. `validate_voucher` and the voucher watermark are the only freshness rules, on
  both books. 0005's first amendment (by 0074) stops being a second rule beside the first and becomes
  the only one.
- **Journal entries for signed claims are kept, now holding vouchers.** `BatchChannelAdmitted` covers
  every inbound channel. **A new entry records an outbound channel's config** — on EVM the whole
  `ChannelConfig`, including `salt`; on Solana the `salt` and the counterparty it was opened toward —
  because nothing else can reconstruct it after a restart. It is neither signed nor irreversible, and it
  is journaled anyway for the reason 0074 gave for `BatchChannelAdmitted`.
- **A journal holding `toon-channel` entries is refused at boot, by name**, with the drain procedure in
  the message (see "Draining a node with live TOON channels"). It is never silently skipped: a skipped
  entry is a claim somebody could still redeem that this node has forgotten it signed.

### 9. Configuration

Configuration stays one typed file with `deny_unknown_fields`
([0009](0009-one-typed-config-file-no-environment-layer.md)), and every removed key is parsed in order
to be refused by name.

- **Refused by name:**
  - `[settlement.evm]` `contract_address`, `channel_index_from_block` and
    `channel_index_confirmations`;
  - `[settlement.solana]` `program_id`;
  - the legacy flat `[settlement]` shape;
  - the `[settlement.<chain>.batch_settlement]` sub-table, whose keys move up a level;
  - `[[client_channels]]`, since an x402 channel is resolved from the chain and the voucher;
  - the EVM `[[peer_channels]]` fields `token_network`, `chain_id` and a derived `channel_id`, and the
    Solana `channel_account` field in its TOON meaning.
- **Moved up into `[settlement.<chain>]`, each keeping its bounds:** `min_withdraw_delay_secs`,
  `asset_eip712_name` and `asset_eip712_version` on EVM; `min_grace_period_secs` and
  `min_sponsored_deposit` on Solana.
- **Newly required:** both EIP-712 keys wherever `[settlement.evm]` exists, and
  `min_sponsored_deposit` wherever `[settlement.solana]` exists. Accepting x402 channels stops being an
  opt-in, because they are the only channels. **That is a breaking deploy for every node repository**
  (the binary and the box's TOML are a matched pair in both directions — 0009,
  [0068](0068-a-node-repository-pins-the-connector-nothing-here-moves-a-tag-onto-a-box.md)), and the
  release notes carry a before-and-after config.
- **`[[peer_channels]]` may still name channels explicitly**: an inbound channel id with its voucher
  signer, and an outbound channel id. Admission by the published key is the default.
- **`[settlement.evm]` then needs only** an RPC URL, a token, its decimals, its asset's EIP-712 domain
  and a key; **`[settlement.solana]`** an RPC URL, a mint, its decimals and a key, plus the minimums.
- `deploy/connector-rust/connector.production.toml` follows the new shape and stays inert
  ([0056](0056-production-is-a-named-empty-tier.md)).

### 10. The greeting and the self-description

This **amends 0074 decision 8** and **[0050](0050-a-connectors-url-resolves-to-its-self-description.md)**.

- **The greeting** drops the `toon-channel` `accepts[]` entry, with its `X402SettlementTerms` and
  `X402SolanaSettlementTerms`. The `batch-settlement` entries, one per configured chain, are the whole
  list, and every one is x402-valid. A stock x402 client can pay any TOON connector.
- **The self-description** drops `settlements` (the TOON channel terms) and keeps `batchSettlements`.
  It also publishes, per chain, the node's **voucher signer** — its settlement address on EVM, its
  settlement key on Solana, which its outbound channels name (decision 3) — as the value a peer binds
  the inbound channel by (decision 4). The announcer sidecar reads channel terms from
  `batchSettlements` alone.

### 11. The operator surface

- **Kept:** `POST /channels` (open an outbound channel to a receiver) and `POST /channels/:id/fund`
  (top up by an increment — already an increment on both chains since #1118).
- **New:**
  - `POST /channels/:id/withdraw` — start a withdrawal (EVM) or request a close (Solana) on an outbound
    channel, and finish it when it is due;
  - `POST /channels/:id/land` — land the held latest voucher on an inbound channel now, replacing
    `redeem-latest`. The watchers and sweeps still land vouchers automatically; this is the manual lever
    for planned maintenance.
- **Removed:** `redeem`, `redeem-latest`, `settle`, `close` and `cooperative-close`.
- **`GET /channels` and `GET /claims`** show each channel's direction (inbound, where this node is the
  receiver; outbound, where it is the payer) with its collateral, watermark and status, and show
  vouchers received and vouchers signed. The dashboard
  ([0066](0066-the-operator-dashboard-is-a-page-the-surface-serves-and-signs-in-the-browser.md))
  follows.
- Every write is RFC 9421-signed ([0008](0008-operator-surface-splits-read-from-write.md)). Automatic
  top-up or rebalancing is **not** added: topping up stays an operator write.

### 12. What is deleted with TOON's channels, and what is kept as history

**Deleted:** the EVM channel index and its syncer (`TokenNetwork` logs), `IndexedEvmChannelSource`
and `SettlementChannelSource`; EVM channel-id derivation; the `TokenNetwork` ABI and its
`abi_provenance` check; `test_support`'s `TokenNetwork` deploy helpers; `SolanaValidator`'s loading of
`payment_channel.so`; the `DeployLocal` script's `TokenNetwork`, registry, `RollingSwapChannel` and
forwarder deployments; the Solana local container's TOON program; `tools/fund-peers`' `TokenNetwork`
calls; and last, `packages/contracts` and `packages/solana-program` themselves, with their Foundry and
`cargo test-sbf` CI jobs and the `make solana-test` target. Each TOON-channel path is deleted in the
step that replaces it, never left beside it.

**Kept:** the deployment records under `packages/*/deployments/` move to `docs/deployments/`. The
contracts and program stay on chain — nothing can delete them — and one third-party operator's
channels live on them until drained.

### 13. The local stack

- **anvil** hosts the x402 contract and its collectors at their canonical addresses, as
  `test_support::X402Chain::place` already does, and Circle's FiatToken v2.2 as USDC, so ERC-3009
  deposits work. `MockERC20` minted on demand is no longer the local token.
- **The Solana container** loads `payment-channels` at `CHNLx…` and mainnet's p-token, from the pinned
  fixtures under `crates/connector-settlement-solana/fixtures/`.
- **`local/keys.sh`'s channel stages** open channels through the running nodes' `POST /peers` and
  `POST /channels`, and through the sponsor endpoint on Solana, instead of `cast` and
  `open-solana-channel.py`.
- `make local-verify`'s peered topologies cross their peerings on x402 channels, more than once, and
  read the payee's journal; `dealing` still asserts the converted figure.

### 14. Vectors: `schema_version` 7

This disturbs [0021](0021-vectors-are-normative-prose-is-not.md). `vectors/wire-vectors.json` drops
every `toon-channel` section — the top-level `claim` cases; `peer_carriage`'s `claim_evm`,
`claim_solana`, `claim_digest_hex` and nonce cases; and `channel_control_declaration`, the
`TokenNetwork`-domain `auth_channel_proof` — and gains a **peer voucher** case, a **zero-value peer
packet** case, and a **peer-role challenge** case. The ack's cases are regenerated for vouchers. `schema_version` goes from 6 to **7**,
and each voucher case is still cross-checked against the deployed `getVoucherDigest`. `toon-client`,
`rig` and `swap` replay it; their implementations are theirs.

## Routing fees are untouched

[0010](0010-flat-per-packet-fee-and-minimum-delivery.md) and
[0061](0061-a-fee-attaches-to-a-peering-not-to-a-route.md) stand exactly as written. A connector that
carries someone else's packet keeps its peering's flat per-packet `fee`, paid by the caller as part of
the path's cost; a terminating operator keeps its route's whole `price`; and `price − fee ≥ next hop
price` still protects the terminating price. There is still no protocol fee. Only the channel the fee
is paid on changes: a hop's earning is still the difference between the cumulative it receives from
upstream and the cumulative it signs downstream — now on two channels instead of one.

## The trust statement, restated for all of a node's value

0074 decision 5's trust statement covered only client-edge value, and only for an operator who opted
in. It now covers **every unit of value a node holds**, with no opt-out:

- **EVM.** The contracts are ownerless and immutable. Nothing can change them and nothing can rescue a
  stuck escrow: a USDC-blacklisted payer or receiver traps a channel for good (Cantina 3.2.1,
  acknowledged). That now includes a node's own outbound collateral toward its peers and its clients.
- **Solana.** `payment-channels` is upgradeable. On mainnet-beta its upgrade authority is a **Squads v4
  multisig vault, 3 of 5**, with no time lock; on devnet it is a **single keypair**. The mainnet binary
  **is** the audited source: a verifiable build of the audited commit reproduces it byte for byte. The
  devnet binary matches **neither** verifiable build of the audited commit that was tried. The facts,
  and how each was established, are under "The `payment-channels` deployment, as found".
- **Audit scope.** Cantina's July 2026 report on `payment-channels` lists no `settle.rs`, `top_up.rs`
  or `request_close.rs` in its scope (0074, decision 5). Under 0074 only a client called `top_up` and
  `request_close`. Under this record a paying node calls both on every channel it funds and winds down,
  and every receiving node lands through `settle`. The code that moves a node's own money is the code
  the audit left out, and nothing here closes that gap.
- **What a node risks.** As a receiver: what it has accepted but not landed. As a payer: its outbound
  deposits, which the program or contract holds and the node recovers only by withdrawing.

TOON gives up owning its settlement code. In return it stops having to deploy, audit and redeploy it.

## Consequences

**Costs, stated rather than hidden:**

- **Collateral roughly doubles on a peering with traffic in both directions**, because nothing nets:
  each direction holds its own deposit and settles on chain on its own. Peers typically carry most
  value toward the terminating node, so the reverse channel is usually small — but it is still funded,
  still costs gas to open and top up, and still costs gas to land.
- **Winding down takes the counterparty's minimum delay.** Getting outbound collateral back is
  `initiateWithdraw` then `finalizeWithdraw` after `withdrawDelay` on EVM, or `request_close` then the
  grace period then `distribute` on Solana. Both default to **one day** (0074 decision 5).
- **Solana rent float grows.** A receiving node pays 4,711,920 lamports (about 0.0047 SOL) for every
  channel opened toward it — peers' as well as clients' — until `reclaim`. `min_sponsored_deposit` and
  the sponsor endpoint's limits bound it; the sweeps bring it home.
- **Client payouts no longer raise spendable headroom** (decision 7). A client earning and spending
  through one connector needs collateral in its own channel and a payout channel funded by the
  connector.
- **`POST /peers` is no longer structurally idempotent.** 0059 made a repeat derive the same channel.
  Here a repeat finds this node's own outbound channel through the journal entry decision 8 adds, which
  is why that entry is written **before** the opening transaction is sent (a choice this record makes;
  see below). A crash between the two leaves an entry that names a channel the chain may or may not
  hold, and the next attempt reads the chain before opening another.
- **The settlement key signs every outbound voucher, on both chains** (decision 3). It already signs
  the node's claims and its settlement transactions, so no key gets hotter than it is. A separate
  session key is admissible (0074 decision 6) and is not taken here. `CLAUDE.md`'s key table said
  `signer.key` signs claims, which was half right: today it signs client-payout claims, while the
  settlement keys sign peer claims. It was corrected on acceptance: `[signer]` is identity (gift wrap,
  the self-description), and each settlement key signs its chain's vouchers and transactions.
- **A challenge is a bearer proof for zero-value traffic until `expires`.** It is bound to one channel,
  so only that channel's receiver can use it, and the peer role it grants moves no value.
- **Gasless onboarding needs an ERC-3009 token.** The devnet already has one: since #1337
  (2026-09-25) its Base Sepolia USDC is a FiatToken v2.2 the faucet mints (see "Prerequisites").
  0074's prerequisite 2 describes the mock ERC-20 it replaced. On mainnet the token is Circle's USDC, which has
  ERC-3009; x402.org's facilitator does not list Base mainnet, so a mainnet client uses another.

**What it buys:**

- **One kind of payment channel**: one pair of on-chain programs to understand and monitor, neither of
  them this project's to deploy.
- **One claim scheme, one freshness rule, one greeting entry type, one settlement port and one set of
  vectors**, for every client and every peer.
- **A client with USDC and no native gas can pay any connector**, on either chain, with a stock x402
  client.
- **Configuring a chain is short**, and a node cannot be pointed at the wrong deployment.

**The largest breaking change since 0042.** It breaks the wire (`schema_version` 7), the config (keys
removed and newly required), the journal (TOON entries refused) and the self-description (`settlements`
removed). Every downstream repository moves with it, and bumping a node repository's pin is that
repository's own reviewed change (0068).

## Draining a node with live TOON channels

The fleet holds none: its production tier is empty (0056) and no devnet node's committed config carries
a peering (0042). One third-party operator's mainnet node does, on the Base mainnet `TokenNetwork` and
the Solana mainnet-beta program. The procedure is **documented, not automated**, and the release notes
carry it:

1. Stay on the **last release that supports TOON channels** (named in the release notes of the first
   release that does not).
2. Land every inbound channel's latest claim (`POST /channels/:id/redeem-latest`).
3. Close and settle every TOON channel the node participates in, cooperatively where the counterparty
   will (`cooperative-close`), otherwise `close` and then `settle` once the settlement window has run.
4. Confirm on chain that no TOON channel of the node's is still open, then upgrade.

A node that upgrades first finds its journal refused at boot, by name, with these steps in the message
(decision 8). Nothing is lost by that refusal: the contracts stay on chain, and the previous release
still drives them.

## The sweep

**Does not survive:**

- **0074 decision 1 whole** (client edge only; a connector never opens, funds or signs; payouts on
  TOON channels; opt-in), and **decision 9's receive-only port**.
- The clauses of 0074 that rest on decision 1: decision 2's "0059 still binds every channel a connector
  opens itself"; decision 3's "it never signs a voucher itself" (the rule it justified, `expiresAt = 0`,
  stands, and the node now signs with it); decision 4's absent-scheme default; decision 5's "an operator
  who opts in … the fleet does not opt in by default"; decision 6's "a voucher never decides the peer
  role"; decision 8's "`accepts[]` keeps its `toon-channel` entry"; and its Considered option rejecting
  x402 everywhere.
- **0024 and 0053** — the `toon-channel` claim schemes. **0059** — derivation, and one live channel per
  pair. **0026's #700 update** — payout netting.
- **0042's one-channel peering**, and **0058**'s "derives the channel, opens it if absent, registers
  both the PEER-role and the CLIENT-role halves of the channel binding".
- **0060's** "a claim on a channel one of that peer's `[[peer_channels]]` rows configures, whose
  signature verifies against the counterparty key that row configures" — replaced by decision 5.
- **0005's** nonce watermark.
- **0050's** `settlements`.
- **0073's** second falsifier (`channel_index_sync.rs`), which becomes vacuous when the syncer is
  deleted; its decision is untouched.

**Survives unchanged:**

- **0074 decisions 2, 3 and 5 on the receiving side** — admission, the amount-only watermark,
  retransmission, the watchers, the sponsor rule — and decision 7's voucher cases. This record is 0074
  extended, not reversed.
- **0042's rule** that a packet carries its claim, **0004's** one claim per packet, **0033**'s
  retirement of exposure, and **0049**'s cap.
- **0060's** principle: a signature proves a peering, and no shared secret does.
- **0058's** one-write onboarding, trust-on-first-use, and its bounds on the outbound fetch.
- **0010, 0061, 0028, 0029 and 0071**: fees, prices and dealing.
- **0052**: permissionless payment. **0022**: the deferral of paying over plain HTTP.
- **0027 and 0070**: carriage. **0018, 0019, 0063, 0069**: the envelope, gift wrap, fulfilment
  derivation and packet dialect.
- **0073**: every settlement client is built from its table's one `RpcTransport`.

## Considered options

- **Keep TOON channels for peering and payouts (0074 as it stands).** Rejected. It keeps both costs
  under "What two kinds of channel cost" in full, for the one reason 0074 gave, which decision 4 answers.
- **Net the two channels of a peering off chain.** Rejected, and out of scope. Netting needs both sides
  to agree a balance, which is exactly the accumulation 0033 retired and 0042 refuses; each packet
  carrying its own claim is what keeps a peering from owing anything between packets.
- **Keep deriving peering channels: a fixed `salt` on EVM.** Rejected, as 0074 rejected it at the
  client edge. It is impossible on Solana, whose PDA includes `open_slot`, and under two one-way
  channels there is nothing shared to land on.
- **Keep `toon-channel` claims readable for a transition release.** Rejected. Two schemes side by side
  is the cost this record exists to remove, and the drain procedure gives a straggler a release to do
  its draining on.
- **Run an x402 facilitator.** Out of scope. A node pays its own gas when it deposits, and clients keep
  using stock facilitators on EVM.

## Decided by the owner before acceptance (2026-09-27)

These were open when the record was first written. The owner decided them on 2026-09-27, before accepting the record the same day.

1. **The voucher signer is the chain's settlement key, on both chains** — on EVM
   `payerAuthorizer == payer`. This reverses #1371's `[signer]` choice. The reasons are under decision 3.
2. **PR #1368 lands first, as it is**, under the current records (see "Prerequisites").
3. **The devnet's Base Sepolia leg runs on an ERC-3009 USDC the faucet can mint** before the first
   release that ships this record (see "Prerequisites").
4. **The clauses of 0074 decisions 2, 3, 4, 5, 6 and 8 that repeat decision 1's scope are superseded**
   here too (The sweep). #1371 named only decisions 1 and 9.
5. **0024, 0053 and 0059 are _Retired by 0075_**, not superseded: their mechanisms are deleted, and what
   does the job — 0074 decision 4's voucher — existed before this record.

## The `payment-channels` deployment, as found

Read on 2026-09-27 from public RPC only. Nothing was signed or sent. This settles the question 0074
left open, and this record first carried as one for the owner, for both clusters.

|                            | mainnet-beta                                                                                                                                                                           | devnet                                                             |
| -------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------ |
| Program / ProgramData      | `CHNLxYvV…yGsX` / `CghQXkmw2F6p1exMETiZdNeUx9QGraWsNZ4eom1Cuiw1`                                                                                                                       | the same two addresses                                             |
| Upgrade authority          | `DXtFpbPjcn2hxPnw79x1Pfoj35vXh5AsWBkS37YnXMVv`                                                                                                                                         | `4zTeC5mVqWLruDexgU2mV66p9t5vCA9JyiZqdGDUspap`                     |
| What the authority is      | **A Squads v4 vault, 3 of 5**: vault 0 of multisig `4CxQs26DewQ1KaCHfyyjktkYjndNdUqCJvVdygtJFwcJ`, threshold 3, 5 members each with every permission, time lock 0, no config authority | **A single keypair**                                               |
| Last deployed              | slot 431447053 (2026-07-07), by keypair `96WoyH3J…KuD`; authority moved to the vault at slot 435599419 (2026-07-27)                                                                    | slot 480232051 (2026-07-31), by the authority keypair itself       |
| On-chain executable hash   | `afe23e2373acb36fa7317b7044beb07d785c2dacd5e0a494586cbabaa67485f2`                                                                                                                     | `6b5b11c1a42b294748b0759a04971cd4f3970092f48d012383fe4af8a110114d` |
| Equals the audited source? | **Yes, established here**                                                                                                                                                              | **Not established**                                                |

**How the authority was established.** `solana program show` gave each authority. Both accounts are
owned by the System Program and hold no data, which on its own does not tell a keypair from a Squads
vault. What does: the mainnet authority is **off** the ed25519 curve, so it is a program-derived
address with no private key, and deriving Squads v4's vault 0
(`["multisig", multisig, "vault", 0]` under `SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf`) for
multisig `4CxQs…` gives exactly `DXtFp…`. That multisig was found from the authority's earliest
transaction, a Squads `MultisigCreateV2`. It is owned by the Squads v4 program, and it is itself the
PDA of its `create_key`. Its account data, read by the Squads v4 layout, gives threshold 3, time lock
0, 5 members with permission mask 7 (initiate, vote, execute), and a config authority of the default
key, so only the multisig itself can change its own members or threshold. Who the five members are
was not determined; they are keys, not named parties. The devnet authority is **on** the curve, so
it is not a program-derived address. It signed the devnet deploy itself: one key, which can replace
devnet's program at any moment.

**How the binary was established.** The on-chain executable hash is `solana-verify
get-program-hash` (v0.5.2); a `solana program dump` hashed the same way, with trailing zeros trimmed,
agrees on both clusters. The mainnet hash is the hash of the committed fixture
`crates/connector-settlement-solana/fixtures/payment_channels.so`, whose whole-file SHA-256 a test
pins as `PAYMENT_CHANNELS_FIXTURE_SHA256`. A verifiable build (`solana-verify build`, image
`solanafoundation/solana-verifiable-build:3.1.13`, the upstream `justfile`'s recipe) of the audited
commit `0c07d575` with `--no-default-features --features mainnet-beta` produced
`afe23e23…85f2`, **the mainnet hash exactly**. The pin 0074 cites, `3ffa4d67`, has no change under
`program/` since `0c07d575`, so the same holds for it. Third-party evidence agrees and is only that:
OtterSec's verification API holds a verify record from `96WoyH3J…KuD`, the deploying key, matching
this hash to `0c07d575`, dated 2026-07-07. Its headline status reads "not verified" because OtterSec
counts only a record from the current upgrade authority, and the vault has written none.

**Devnet's binary is not the audited source as far as anything here can show.** A verifiable build
of `0c07d575` with default features — the only feature set that compiles with the placeholder
`TREASURY_OWNER`, which the fixture's provenance note in `connector-settlement-solana/src/test_support.rs`
says devnet's binary carries — gives `dbfdc5f1…92db`, not devnet's hash, and a `devnet`-feature build
of that commit is refused by its own build-time assert on the placeholder. No later commit touches
`program/`. So devnet runs a build of unknown provenance, under a single key.

**What this means for this record.** Mainnet — where a third party's value sits — runs the audited
source under a 3-of-5 multisig with no time lock: an upgrade needs three of five keys and takes
effect at once, with no window in which a node could withdraw first. Devnet is weaker on both
counts, and devnet is where this repository's own nodes settle. Neither changes the decision; both
are what an operator on either cluster accepts.

- **Still open:** what devnet's binary was built from.

## Prerequisites

- **PR #1368 (#1364's voucher claim-state challenge) lands first, as it is, under the current
  records** — met: it landed on `main` as `5d6b8f2a`. It reads only the client-edge book, "because a
  voucher is never a peer claim (ADR 0074 decision 1)", which was right until this record was
  accepted. Decision 5 makes its message the peer-role proof and decision 6 makes its answer the
  outbound watermark's authority, so the peering step (step 4 under "Order of work") widens the
  challenge from the client-edge book to the peer book, citing this record.
- **Before the first release that ships this record, the devnet's Base Sepolia USDC is an ERC-3009
  token the faucet mints** (the owner moved this from "before any code" to "before the release" on
  acceptance) — **already met, by #1337.** Since 2026-09-25 the devnet USDC is Circle's FiatToken
  v2.2 bytecode deployed by this project at `0x0C996d7c934c79a6255254875607Fe69df25C0E1`, the same
  shape the local anvil stack uses (decision 13), and every devnet node and the faucet point at it.
  Read on chain on 2026-09-27: `name()` is `USDC`, `version()` is `2`, it answers
  `RECEIVE_WITH_AUTHORIZATION_TYPEHASH()`, and the faucet key `0x7eC0c44F…0dBd` is a minter with an
  effectively unlimited allowance. It keeps "mint, never drip" and gives a devnet client gasless
  ERC-3009 deposits, so "USDC only, no gas" holds on devnet with no exception. A node may deposit
  through ERC-3009 or through Permit2 after a one-time `approve`, since it holds ETH anyway. Minting
  is **minter-gated** rather than open, which the owner's condition allows: what it requires is that
  the faucet can mint, and Circle's own Base Sepolia USDC is rejected because the faucet cannot.
  #1337's remaining follow-ups (the `RollingSwapChannel` redeploy, which this record retires anyway,
  and sandbox parity in toon-protocol/infra) do not bear on it. This record, first written, said the
  devnet's USDC had no ERC-3009; that was 0074's prerequisite-2 finding, overtaken two days earlier.
- **#1367 (scoped log queries for the withdrawal watch)** — met: closed by PR #1370. The watchers this
  record extends to every channel depend on it.

## Choices this record makes beyond #1371

- **The outbound channel's journal entry is written before the opening transaction is sent**, so a
  retried `POST /peers` or `POST /channels` finds it (Consequences). On Solana, where the PDA includes
  `open_slot`, the node rediscovers the channel from the chain by its own `payer` key and the `salt` it
  journaled.
- **A peer-role challenge's `expires` is to be short**, since within it the challenge is a bearer proof
  (decision 5). The bound is the implementing ticket's to fix.

## Order of work

Each step keeps the workspace gate green, and deletes the TOON-channel path it replaces.

1. This record, accepted.
2. The port's paying half and its contract suite.
3. Domain: delete the nonce rules and require `scheme`.
4. Peering and the peer role.
5. `[[pay_channels]]`.
6. Client payouts.
7. Config, greeting and self-description.
8. The operator surface.
9. Vectors (`schema_version` 7).
10. The local stack and topologies.
11. Delete TOON's contracts, program and the dead code.
12. README and docs: `CONTEXT.md`, `client-edge-spec.md`, `peer-carriage-spec.md`,
    `configuration-spec.md`, `self-description-spec.md`, `operator-spec.md`, and `CLAUDE.md`'s key,
    funding and local-stack sections.

## On acceptance

Applied with the acceptance (2026-09-27).

**Status lines updated**, each naming this record:

- 0074: _Partly superseded by 0075_ — decision 1 and decision 9's receive-only port, and the clauses
  listed under "The sweep".
- 0024 and 0053: _Retired by 0075_.
- 0059: _Retired by 0075_.
- 0026: its #700 netting retired by 0075; "one gate, two carriages" stands.
- 0042, 0058, 0060, 0005 and 0050: _amended by 0075_, with an `## Update` section each.
- 0021: _disturbed by 0075_ (`schema_version` 7).

**Glossary** (`CONTEXT.md`), applied on acceptance:

> **Payment channel**: A one-way agreement, anchored on a chain, by which a payer escrows value for one
> receiver and hands it over many times while touching the chain only to open, top up, land and
> withdraw. Always an x402 `batch-settlement` channel. **Named by the voucher that pays on it and
> verified on chain**, not derived: a pair may hold several. A peering is two, one each way.
>
> **Claim**: A signed statement of a payment channel's cumulative state, handed from payer to payee.
> Each claim supersedes the last, so a lost claim costs nothing and a replayed claim gains nothing.
> Every claim is a **voucher**.
> _Avoid_: receipt, payment, balance proof; `toon-channel` (retired, ADR 0075)
>
> **Voucher**: A claim under x402's `batch-settlement` scheme (ADR 0074, ADR 0075) — the only kind.
>
> **Watermark**: The highest cumulative amount a payee has accepted on a channel, which the next
> voucher must strictly exceed.
>
> **Nonce** _(retired term, ADR 0075)_: The counter that ordered `toon-channel` claims. A voucher has
> none; its amount orders it.
>
> **Peer role**: … an interaction is a `peer` only if it carries a voucher on a channel bound to that
> peering, or — for a packet that moves no value — a claim-state challenge signed by such a channel's
> voucher signer …
>
> **Settlement backend**: The chain-specific implementation of the settlement port for one chain:
> opening, funding, signing and withdrawing on its outbound channels, and admitting and landing
> vouchers on its inbound ones.
