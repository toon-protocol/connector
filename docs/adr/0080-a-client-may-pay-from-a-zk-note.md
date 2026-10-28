# A client may pay from a zk note

**Status:** Accepted (owner decision, 2026-10-16, #1511). **Spike #1512 answered yes** (2026-10-16, [research note](../research/zk-note-verifier-spike.md)), so the condition is met and the record stands. **Amended 2026-10-28** (owner decision, #1513): decision 7 names the asset and its rate, and the vault holds one asset with no merge or top-up of notes. **Amended 2026-10-28** (owner decision, #1555): decision 12 makes toon-protocol/zk_toon's `zk-note` crate the definition of every zk-note derivation, and decision 3 gains the checks that crate's request proof leaves to the zk edge. **Amended 2026-10-28** (owner decision, #1556): decision 13 adds the mutual close, the zk edge's clearance and the ownership proof it is given against. **Amended 2026-10-28** (owner decision, #1581): decision 14 admits subsidised allowances (zk_toon's ADR 0001), paid from the zk edge's subsidy budget. Nothing is built. It **amends** [0040](0040-a-verified-payment-is-stated-to-the-app.md): the payer is stated only when it is known. It **adds a note to** [0033](0033-the-exposure-machinery-is-retired-not-restated.md): the risk that record retired comes back at a zk edge, under a cap, as the **open budget**. It leaves [0002](0002-drop-mina-from-the-rust-connector.md) and [0065-mina](0065-mina-leaves-the-repository.md) standing, and the sections below explain why. It disturbs [0021](0021-vectors-are-normative-prose-is-not.md): the allowance claim, the allowance request and answer, the signed successor state, and the clearance request, answer and signed bytes all become vectors.

**Scope:** protocol law. It adds a claim scheme, a greeting entry and three client-edge endpoints that every implementation of a zk edge must speak. Where the proof is verified and how the journal records it are connector architecture. See the [ADR index](README.md).

**Falsifier:** `**/Cargo.toml` matching `o1js` — the connector verifies proofs and never produces them; proving, collection and challenging stay outside this repository, so an o1js dependency here means the line this record draws has been crossed.

**Falsifier:** `**/package.json` matching `"o1js"` — the same line, crossed from the npm side.

**Falsifier:** `crates/**/*.rs` matching `"toon\.zk\.` — a zk-note domain tag written out in the connector means a derivation was defined here instead of taken from zk_toon (decision 12).

**A connector may accept payment from a zk note, and one that does is a zk edge.** A payer
deposits once into the zk edge's Mina vault. It then opens an **allowance** with one proof that
some unspent note in that vault covers a budget from a published tier, bound to this zk edge, a
fresh session key and an expiry. It pays each packet with an **allowance claim**: a cumulative
amount signed by the session key, under a voucher's rules. When the allowance closes, the zk edge
journals its charge Δ once and signs the note's **state advance**: the old balance less exactly
Δ. Past the zk edge nothing changes. It pays every downstream hop with its own vouchers.

## Context

A payer can hide **what** it sends, because the gift wrap is sealed to the termination
([0018](0018-a-payload-is-sealed-to-the-terminating-connector.md)). It can't hide **who** is
paying. Every voucher names the payer's channel, so the first connector links everything that
payer sends, and the chain links the channel to an address. [0040](0040-a-verified-payment-is-stated-to-the-app.md)
keeps the payer from hops past the first, but only for as long as the first hop and the
termination never compare notes. A hidden service ([0070](0070-an-onion-address-is-a-host-not-a-carriage.md))
hides where a node is, not who paid it.

The design adapts zkAPI (`vbuterin/zkapi`, its `zkapi-v2-concession-v1` design): notes, a nullifier
chain with derived successor anchors, one fixed budget per state, settle-then-sign, and the
concession exit. Mina's proofs replace its Groth16 over BN254, so no trusted-setup ceremony is
needed. A TOON price is fixed before a packet leaves, so Δ is known exactly when the allowance
closes, which zkAPI cannot say.

## Decisions

1. **The scheme is `zk-note`, and its claim carries no `blockchain`.** The decoder checks
   `blockchain == "mina"` first and refuses it by name, as it does today. It then branches on
   `scheme`. The `zk-note` arm splits off before the voucher arms require `blockchain`, and a
   `zk-note` claim that carries one is refused as malformed. The `mina` refusal owed to
   `toon-client` therefore doesn't change.
2. **An allowance claim follows a voucher's rules.** Its amount must be strictly above the
   allowance's watermark, the increase must cover the packet's charge, and the amount must not
   exceed the budget. On a reject from the next hop, **the watermark rolls back**, exactly as a
   voucher's does. Δ is the final watermark. The payer checks that Δ is at most the highest
   amount it signed and that the successor equals its old balance less Δ.
3. **One proof per allowance, never one per packet.** Admission is cheapest refusal first:
   structure, tier, expiry, the binding tag, a known root, an unseen nullifier (locally and on
   chain), and the proof. Nothing is written until every check has passed.
   _Amended 2026-10-28 (owner decision, #1555):_ the proof's public inputs are those of zk_toon's
   request proof: the state key, root, request time, tier, nullifier, authorization tag,
   re-randomised commitment, subsidised flag, period and per-period nullifier. The tier is the
   budget `b`. The proof binds the zk edge, the session key and the expiry only through the tag,
   and it checks the note's expiry only against the request time the payer supplies. So, before
   the tag, admission also refuses by name:
   - a state key other than the zk edge's own;
   - a request time more than 10 minutes behind or 1 minute ahead of the zk edge's clock. The
     window is a constant, not config, because every wallet has to know it. Without it a payer
     could backdate a request and spend a note that has already expired;
   - a subsidised flag, period or per-period nullifier that is set. _Amended 2026-10-28 (#1581):_
     decision 14 replaces this refusal with the subsidised checks.

   The zk edge enters the tag as the x-coordinate of its state public key, which the greeting
   already publishes. An allowance claim is signed by the session key with the chunked nonce
   under the devnet domain (`NetworkId::TESTNET`), as the state is. The tiers the table declares
   must be a subset of the crate's tiers, and a table naming any other tier refuses to boot.

4. **Close is final, and it waits for packets in flight.** An allowance closes on a signed
   request, on expiry, or when it reaches its budget. Closing stops new claims at once. Δ is
   journaled only after every forward already in flight has resolved, which packet expiry bounds.
   Until then a successor fetch answers "pending". A sweep closes expired allowances, and it also
   runs once at boot after replay. A successor fetch only reads.
5. **Open budget is capped.** The sum of the budgets of open allowances may not exceed a cap the
   operator sets. An allowance that would exceed it is refused by name. This is the one place the
   risk [0033](0033-the-exposure-machinery-is-retired-not-restated.md) retired comes back. An
   allowance claim covers its packet with a proof, not with escrow the payee holds, until the note
   is collected, and a dishonest escape the zk edge never challenges keeps money it has already
   paid out downstream. The word **exposure** stays retired.
6. **The payer is stated only when known.** For a packet paid by an allowance claim, the app gets
   `X-TOON-Amount` and `X-TOON-Chain`, the Mina network, and does not get `X-TOON-Payer`. This
   amends [0040](0040-a-verified-payment-is-stated-to-the-app.md). The payment was verified, so it
   is stated; only who paid is unknown.
7. **A zk edge prices in the vault's asset.** Its route prices for allowance-paid packets are in
   that asset. Its forward to the next hop crosses a denomination at a declared rate
   ([0071](0071-a-forward-crosses-a-denomination-at-a-declared-rate.md)). The Mina asset is accepted
   only as the zk-note table's own; a `mina:` asset in `[[tokens]]` or a settlement table is still
   refused by name. _Amended 2026-10-28 (owner decision, #1513):_ the vault's asset is native MINA,
   9 decimals. The table declares it, and has two spellings for it:
   - **In the greeting**, the zk-note entry follows the x402 entries' shape: `network` is the
     CAIP-2 network (`mina:mainnet` or `mina:devnet`), and `asset` is `slip44:12586`, MINA's
     SLIP-44 coin type. Together they read as the CAIP-19 `mina:mainnet/slip44:12586`. That
     string is TOON's own composition: the `mina` namespace has no CAIP-19 profile
     ([research](../research/mina-tokens-and-stablecoins.md)).
   - **Inside the connector**, it is the `AssetId` `mina:native`. A `[[rates]]` row may name it;
     `[[tokens]]` still may not.

   No pool a TWAP source can read quotes MINA, so the rate out of it is a declared `[[rates]]`
   row, static for the process lifetime. The operator covers MINA's movement between restarts
   with a spread, and the open-budget cap bounds what rides on that rate.

8. **Allowances are opened over HTTP only.** `POST /ilp/zk-note/allowances` opens one, or closes
   one when signed by its session key. `GET /ilp/zk-note/allowances/:id/successor` returns the
   signed successor and is idempotent. There is no BTP frame for either. Allowance claims ride both
   carriages, as vouchers do.
9. **Mina reads follow ADR 0073's rules.** The zk edge's reads of vault state (known roots,
   nullifiers used on chain) come from one `connector_chain_rpc::RpcTransport` per table. When
   the table sets `rpc_via_socks_proxy`, they ride the node's `socks_proxy` on a pinned circuit
   of their own (SOCKS username `toon-zk-note-mina`), with bounded timeouts and no fallback to a
   direct connection. A failed or slow read refuses that one allowance by name; it never takes
   the node down ([0073](0073-settlement-rpc-may-ride-the-circuit-once-every-wait-on-it-is-bounded.md)).
10. **A zk edge need not be a hidden service.** It may serve on a public address or an onion
    one ([0070](0070-an-onion-address-is-a-host-not-a-carriage.md)). Its location is the
    operator's to protect; the payer's privacy comes from the proof and from the payer's own
    circuits, not from where the zk edge is.
11. **Durability comes before every answer.** Proofs go to their own append-only evidence file,
    fsynced before the grant. The journal records the evidence's hash, the spent nullifier, the
    allowance terms and watermarks, the Δ written once, and the signed successor. Open allowances
    are restored on restart.
12. **The derivations are zk_toon's, and the connector pins them.** _Added 2026-10-28 (owner
    decision, #1555)._ Every value that a payer, the vault's proof program, the challenger and a
    zk edge must agree on is defined by the `zk-note` crate in
    [toon-protocol/zk_toon](https://github.com/toon-protocol/zk_toon), next to the proof program
    that checks it. That covers the domain tags, the note commitment and leaf, the anchors, the
    nullifier, the authorization tag, the per-period nullifier, the balance commitment's
    generators and the state message. The evidence file's format is zk_toon's too, because its
    challenger is the reader. The connector depends on that crate pinned by git commit and
    defines none of these values itself. The crate's `test-vectors/zk-note.json`, at the pinned
    commit, is the parity contract. A derivation changes only in zk_toon. A pin bump is a
    reviewed change in this repository that replays those vectors, and a bump that changes a
    derivation breaks every note in a vault. The connector's own wire stays here, in
    `vectors/wire-vectors.json` ([0021](0021-vectors-are-normative-prose-is-not.md)), with its
    values computed through the pinned crate: the allowance claim and its signed bytes, the
    allowance request and answer, and the signed successor. The crate and the verifier share one
    `proof-systems` source, the one the live Mina daemon pins. For Mesa that is tag `0.7.0`.
13. **A zk edge clears any note its owner proves, and a state is spent once.** _Added 2026-10-28
    (owner decision, #1556)._ A payer leaves at once by **mutual close**, carrying the zk edge's
    **clearance**: its signature letting one state leave to one destination. The payer gets it from
    `POST /ilp/zk-note/clearances` (HTTP only, mounted only with the zk-note table, and with no
    operator auth, because the **ownership proof** is the authentication) by showing an ownership
    proof: a withdrawal proof of zk_toon's ownership kind, which the vault refuses and only the zk
    edge reads. It is checked against its own verification key, a second pin the zk-note table
    requires. A zk edge that cannot clear leaves its payers only escape.
    - **No discretion.** The zk edge signs a clearance for every request that passes and refuses
      only by a named rule. Withholding one gains the operator nothing and costs the payer the
      challenge window and up to one allowance.
    - **Cheapest refusal first, and nothing written until every check passes:** structure,
      including that the destination is a Mina public key; a proof bound to this zk edge's state
      key; `requestTime` inside the same window as decision 3's; a known root; the state's
      nullifier unspent, locally and on chain; then the proof. Note expiry is the vault's to
      refuse, not the zk edge's.
    - **One spent-set.** A state's nullifier is spent once, by an allowance or by a clearance,
      and whichever came first refuses the other by name. An allowance already open on a state
      has spent its nullifier, so that state cannot be cleared; its payer waits for the successor
      and clears that.
    - **Journaled before it is signed.** The journal records the nullifier, the destination and
      the clearance, fsynced before the answer. A repeat request for the same nullifier and the
      same destination re-verifies the proof and returns the same clearance, writing nothing, so a
      lost answer never forces an escape. A different destination is refused by name.
    - **The ownership proof is not kept.** Nothing reads it afterwards: the challenger's evidence
      is for allowances. Clearances add no line to the evidence file, and logs and metrics never
      carry a nullifier or destination in plaintext.
    - **The message is zk_toon's** (decision 12), and must bind the nullifier and the
      destination. The ownership proof's public inputs must bind the zk edge's state key and a
      request time, so that one proof cannot be shown to another zk edge or replayed later. The
      greeting's `zk-note` entry names the clearance endpoint and the withdrawal proof's
      verification key hash.
14. **A subsidised allowance is paid from the subsidy budget.** _Added 2026-10-28 (owner
    decision, #1581; the shape was set in #1511's design review)._ zk_toon's
    [ADR 0001](https://github.com/toon-protocol/zk_toon/blob/main/docs/adr/0001-a-notes-reward-is-service-not-money.md)
    lets a note at least 7 days old open one smallest-tier allowance per **subsidy period**, and
    its request proof proves the age, the period and the **per-period nullifier**. The connector
    computes none of them: the age and the nullifier need the note's private deposit time and
    secret. It owns who pays the charge, the per-period nullifier's uniqueness, and the budget.
    - **Admission.** A subsidised proof is admitted under the same verification key as any
      other, after decision 3's checks. Before the binding tag, it is also refused by name, in
      this order, when:
      1. its tier is not the smallest of zk_toon's tiers;
      2. its period is not the current one, as `zk_note::policy::subsidy_period` computes from
         the vault's creation time and the zk edge's clock. There is no grace window at a period
         boundary; the wallet proves again for the new period;
      3. the period's subsidy budget is closed;
      4. its per-period nullifier has already been seen.
    - **Per-period nullifiers are the zk edge's alone.** The vault never records them, so they
      are checked in the journal only, never on chain. Each is journaled durably before the grant
      (decision 11). One from a finished period may be pruned, because its proof could no longer
      be admitted.
    - **The budget is counted in the vault's asset, per period.** The table's `subsidy_budget`
      is an amount in MINA base units, zero by default. A table with a non-zero budget must offer
      the smallest tier, or it refuses to boot, and only then does it read the vault's creation
      time, once, at boot; a failed read fails the boot. A grant **reserves** the full tier
      against the budget of the period it was granted in. The close **charges** Δ instead, and
      the remainder returns to that period's budget, or is lost if that period has ended. The
      budget is **open** while what remains covers the smallest tier, so a subsidy is never
      partial.
    - **The note is charged nothing.** The state advance still happens, so the note's state moves
      on and stays unlinkable, but the successor carries the old balance unchanged. The payer
      checks it against a charge of zero; Δ is the budget's. The vault must accept a state
      advance with zero charge (zk_toon #12).
    - **A subsidised allowance counts toward the open budget** (decision 5): the zk edge pays
      downstream for it exactly as for any other.
    - **What is published.** The greeting's zk-note entry gains `subsidy`, `"open"` or
      `"closed"`, and omits it when the budget is zero. It never says how much remains. A wallet
      that read `"open"` may still be refused as closed; the refusal is authoritative. The
      self-description publishes how many subsidised allowances were granted in the last
      **finished** period, never the current one, and no amount.
    - **Vectors.** A subsidised allowance request, its successor with zero charge and the
      greeting's `subsidy` field join `vectors/wire-vectors.json`. The budget, an amount, stays
      out of them, as the asset does.
    - **Known leak, accepted.** The zk edge sees that an allowance is subsidised, so it learns
      that the payer's note is at least 7 days old.

## Why this does not undo 0002 or 0065-mina

[0002](0002-drop-mina-from-the-rust-connector.md) rejected Mina as a **settlement chain** because
its zkApp methods need **proving** through o1js. It rejected reimplementing the zkApp's proof program against
Rust kimchi/pickles because a verification-key mismatch "is undetectable until a transaction is
rejected on-chain". Neither applies here:

- The connector **verifies** proofs that o1js produced elsewhere. It proves nothing, and it sends
  no Mina transaction.
- A mismatch is caught **at test time**: the port's contract suite has to accept a committed
  fixture that o1js produced, pinned by hash. It isn't discovered on chain.
- There is still no Mina channel, no Mina settlement backend, and no sidecar.

[0065-mina](0065-mina-leaves-the-repository.md) deleted the zkApp, its tooling and the faucet's
Mina leg. None of that comes back. The vault's proof program, the payer's wallet, collection and the
challenger all live outside this repository. The rule "do not reintroduce an o1js dependency" in
`CLAUDE.md` stands, and this record's falsifiers enforce it.

## Considered options

- **A proof per packet.** Rejected: it puts proving time on every request, and verification on
  the packet path.
- **A separate charged total that rolls back while the watermark doesn't** (the first draft of
  #1511). Rejected: it gives allowance claims a second rule set, and the payer can't check it any
  better than a watermark, because Δ is declared by the zk edge either way.
- **Withholding `Amount` and `Chain` as well as `Payer`.** Rejected: the amount is the route's own
  price and leaks nothing about the payer, the chain is all but implied by the node, and apps that
  meter by amount would break.
- **Checking in the connector that a challenger is alive.** Rejected: it would make the connector
  depend on software outside this repository. The cap on open budget is how the operator limits
  the damage.
- **Proof evidence in the journal.** Rejected: proofs are tens of KB and are evidence, not money
  state. The journal keeps a hash of each one.
- **The connector's own primitives crate** (#1521). Rejected (2026-10-28, #1555): it was built
  at the same time as zk_toon's and disagrees with it on every derivation. The vault's proof
  program is written against zk_toon's, so a zk edge built on the connector's would refuse every
  real allowance at the tag, and the vault would refuse its successors.
- **Merging the two crates into one, in one place.** Rejected (2026-10-28, #1555): the proof
  program, the challenger and the wallet already build on zk_toon's crate, and a zk edge only
  checks against them.
- **Merging notes.** Rejected (2026-10-28): one proof spending two states needs somewhere to record
  the result. On chain, the vault cannot see the nullifiers a zk edge journaled off chain, so a
  merged leaf would have to wait out a challenge window like an escape. At the zk edge, the merge
  would be a second proof type in the connector and would tell the zk edge that two states share
  an owner. A payer whose notes are each too small for a tier withdraws and deposits again.
- **Topping up a note.** Rejected (2026-10-28) for the reason in Consequences: it links a public
  deposit to a private state.
- **The CAIP-19 string as the `AssetId`.** Rejected (2026-10-28): every `AssetId` is
  `<chain>:<token>` with no network in it, and the zk-note table already fixes the network.
- **An operator-writable MINA rate first.** Rejected for now (2026-10-28): ADR 0071 names it as
  the natural next step, but it is a change of its own and need not precede the zk edge. A
  static rate with a spread, under the open-budget cap, is enough to start.
- **Charging the subsidy budget the full tier at grant.** Rejected (2026-10-28, #1581): the budget
  would pay for service never given, and the owner's design says it is charged the amount used.
- **Charging the subsidy budget only at close.** Rejected (2026-10-28, #1581): allowances already
  open could spend past the budget, which "no partial subsidy" forbids.
- **A grace window at a subsidy period's boundary.** Rejected (2026-10-28, #1581): it would be a
  second rule that every wallet must compute, to save one re-proof a week.
- **Checking per-period nullifiers on chain.** Rejected (2026-10-28, #1581): the vault does not
  record them, so there is nothing on chain to check.
- **Publishing subsidy counts for every finished period.** Rejected (2026-10-28, #1581): the
  journal would grow without bound, for history anyone watching can keep for themselves.

## Consequences

- **A zk edge can't earn on its own.** Collecting Δ from the vault is a Mina transaction, so it
  needs o1js, which this repository forbids. The operator runs a collector and a challenger from
  outside this repository. Without the collector the zk edge is never paid; without the challenger
  a dishonest escape keeps money it has already spent. A runbook under `docs/operators/` says so.
- **The payer owes one overlay circuit per allowance.** "Circuit" here and throughout this
  repository means an overlay circuit (Anyone), never the proof program. The proof stops the zk
  edge linking allowances by note; only the network can stop it linking them by connection. A
  payer that opens allowance two on the circuit or kept-alive connection it used for allowance
  one hands the zk edge that link, and a payer with no overlay at all hands it its IP address,
  which links every allowance it ever opens. So a payer opens each allowance on a new
  connection under a new SOCKS credential. The connector cannot enforce this; the payer's wallet
  in `toon-client` must.
- **What still leaks:** the timing and volume of allowances, the tier chosen, each packet's
  destination address, the linkage of packets within one allowance, and the last allowance before
  an escape. The anonymity set is the active notes in one vault, so a young vault hides little.
- **A vault holds one asset, and an allowance spends one note** (owner decision, 2026-10-28). The
  note commitment carries no token ID, so a vault holds exactly one asset, native MINA here; a
  vault for any other asset is a different zkApp, under a different verification key and asset
  string. A payer who deposits again makes a **new note**, never a larger old one: a deposit is
  public, and adding it to an existing note would link the payer's address to a state it has been
  spending privately. Budgets stay a published set of tiers, and deposits come in the vault's
  fixed sizes ([toon-protocol/zk_toon#6](https://github.com/toon-protocol/zk_toon/issues/6)); a
  wallet splits any other amount into several notes.
- **The asset stays out of the vectors.** ADR 0071 keeps `vectors/wire-vectors.json` unit-silent,
  and its falsifier refuses any `asset` or `token` key there. The zk-note vectors pin the claim,
  the allowance exchange and the successor; the asset is stated in the greeting.
- A connector without the zk-note table refuses `scheme: "zk-note"` like any unknown scheme, and
  behaves exactly as before.
- Built in two issues, unblocked by #1512 (answered yes): the gate, journal, config, greeting and
  vectors over an in-memory vault, on zk_toon's primitives, then the Mina adapter run against
  committed fixtures. The first also reduces `connector-zk-note` (#1521) to a boundary that
  re-exports zk_toon's crate and keeps only the session-key signature check and tier validation.
  Subsidised allowances (decision 14) are a third piece of work, #1581: they wait for zk_toon's
  subsidised request proof (zk_toon #12), and until it lands admission refuses them.
