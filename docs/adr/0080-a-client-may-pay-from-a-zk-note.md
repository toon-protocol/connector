# A client may pay from a zk note

**Status:** Accepted (owner decision, 2026-10-16, #1511), **conditional on spike #1512**. Nothing is built. If #1512 finds that the connector can't verify an o1js `ZkProgram` proof in Rust, this record is **withdrawn**, not amended, because the route back to o1js is closed (below). It **amends** [0040](0040-a-verified-payment-is-stated-to-the-app.md): the payer is stated only when it is known. It **adds a note to** [0033](0033-the-exposure-machinery-is-retired-not-restated.md): the risk that record retired comes back at a zk edge, under a cap, as the **open budget**. It leaves [0002](0002-drop-mina-from-the-rust-connector.md) and [0065-mina](0065-mina-leaves-the-repository.md) standing, and the sections below explain why. It disturbs [0021](0021-vectors-are-normative-prose-is-not.md): the allowance claim, the allowance request and answer, and the signed successor state all become vectors.

**Scope:** protocol law. It adds a claim scheme, a greeting entry and two client-edge endpoints that every implementation of a zk edge must speak. Where the proof is verified and how the journal records it are connector architecture. See the [ADR index](README.md).

**Falsifier:** `**/Cargo.toml` matching `o1js` — the connector verifies proofs and never produces them; proving, collection and challenging stay outside this repository, so an o1js dependency here means the line this record draws has been crossed.

**Falsifier:** `**/package.json` matching `"o1js"` — the same line, crossed from the npm side.

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
   refused by name.
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
- A connector without the zk-note table refuses `scheme: "zk-note"` like any unknown scheme, and
  behaves exactly as before.
- Built in two issues, both blocked on #1512: the primitives, gate, journal, config, greeting and
  vectors over an in-memory vault, then the Mina adapter run against committed fixtures.
