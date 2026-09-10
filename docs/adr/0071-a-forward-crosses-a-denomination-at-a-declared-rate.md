# A forward crosses a denomination at a declared rate

**Status:** Accepted — not yet built (owner decision, 2026-09-10, issue #1286). Amends
[0010](0010-flat-per-packet-fee-and-minimum-delivery.md): the flat per-packet fee, its earnings
rule and cost discoverability all stand untouched — what falls is the corollary that no rate may
exist at a hop ("value conversion is the `swap` repository's job"), stated in that record's
reasoning, in `local_topologies_load.rs`'s mixed-chain doctrine, and in the RFC 0027 profile's
PF-12/PF-13 bullet, all three of which this record rewrites. Amends
[0061](0061-a-fee-attaches-to-a-peering-not-to-a-route.md) in one clause: at a converting hop the
fee a forward subtracts is denominated in the **outgoing** leg's unit, which is the peering the
fee already attaches to. Extends [0011](0011-rejects-accumulate-fees-and-probes-discover-cost.md):
a reject's running cost now converts at each boundary it crosses, so a probe still answers with
one number in the prober's own unit.

**Scope:** protocol law for decisions 1, 2, 7 and 8 — they bind every implementation and change
what the numbers on the wire mean. The sourcing machinery (decisions 3–6) is connector
architecture, internal to this codebase.

**Falsifier:** `crates/connector-domain/src/**/*.rs` matching `\bf(32|64)\b` — decision 4 claims the conversion arithmetic is integer-rational only, so a float reaching the money path disproves it.

**Falsifier:** `crates/**/*.rs` matching `slot0` — decision 3 claims no spot read exists anywhere in this connector; a pool is read by TWAP over a window the operator set, or it is not read at all.

**Falsifier:** `vectors/wire-vectors.json` matching `"([a-z_]*(asset|denomination)[a-z_]*|token|token_(address|mint|id|decimals))"` — decision 7 claims the wire stays unit-silent in both directions, precisely because every number is kept in the unit of the leg it is on.

A packet's amount has no unit of its own: it is denominated by the channel it rides, which is RFC
0027's own definition ("local amount, denominated in the minimum divisible unit of the asset of
the bilateral relationship"). A **path** is therefore made of **segments** — maximal runs of hops
sharing one denomination — and the invariant is **one segment, one unit**. A hop whose incoming
and outgoing channels hold different tokens sits on a **denomination boundary**, and this record
makes crossing it core forwarding behaviour: the hop converts at a **rate it has declared**,
earns a **spread** besides its flat fee, and refuses the forward when no rate is declared. Every
connector has the capability; no node class, flag or special build exists. What no connector ever
does is convert silently.

## Context

Issue #1286 named two gaps. Forwarding could not cross a denomination — `amount_after_fee` is one
`checked_sub` in one implied unit, and a node holding a USDC channel on one side and an ANYONE
channel on the other forwards the same integer across both, wrong by the scale difference alone
(10¹² between 6 and 18 decimals) before any market price is even asked about. And a probed price
named no denomination — `accumulated_cost` is a bare integer summed across hops that could sit on
different tokens.

Both gaps were held open on purpose. ADR 0010 deleted the percentage spread; the mixed-chain
topology test asserted "STILL NOT A CONVERSION … value conversion is the `swap` repository's job
… the chain boundary in the middle is exactly where somebody would be tempted to put one"; and
the RFC 0027 profile recorded the departure from the RFC's converting hop as deliberate. The
exile rested on an assumption that no longer holds: that conversion had a home elsewhere. The
`swap` repository's maker runs an embedded connector from the retired TypeScript 3.x line,
direct-dialed by clients, deliberately outside every routing table ("no `[[peers]]`/`[[routes]]`
entry … names this service, and none should"). Keeping conversion out of the connector preserved
a mechanism this project has already stopped maintaining.

The alternative was designed in full before being rejected: a converting **app at a termination**
(working name "bureau") — the buyer's packet terminates at a route whose app holds the far-side
float and pays onward as an ordinary toon-client. It works with zero connector changes and keeps
every path single-denomination. The owner rejected it because it makes conversion a product
rather than plumbing: every offering needs a second termination, a mirrored catalog of route rows
on every edge that resells it, and an onward round trip nested inside the buyer's packet
deadline. The owner's call is that these are just ILP hops — a connector that takes in USDC and
routes onward under an ANYONE covering claim is buying one token and selling another, and that
dealing is the connector's own business and profit.

**What ILPv4 does, and what substitutes for its police.** The RFC's model is exactly this —
each connector rewrites the amount at its local rate ("Forwarding, Not Delivery") — but it is
safe there only end-to-end: STREAM lets the sender set a per-packet floor the receiver enforces,
and the sender retries elsewhere when a hop quotes badly. TOON has no transport layer and retired
the declared floor (ADR 0057), so this record must say what substitutes. Three things do. The
sender's own edge is still deterministic: a client pays its edge's posted price in its own unit
(ADR 0028) and the packet amount equals the charge, so no buyer ever interprets a far-side
number. Rates are **declared or absent** — a hop can quote badly only in config it published,
never invisibly, and an undeclared pair refuses rather than guesses. And the residual risks —
staleness, pool manipulation, the free option on a held packet — land on the dealing operator,
whose spread is the compensation and whose guards (decision 5) are the mitigation. Payment stays
per attempt (ADR 0042): a covered packet that a boundary rejects is still spent. That is the
system's existing law, not a new cost of this record.

## Decision

1. **Conversion is core forwarding.** When a forward's incoming and outgoing channels hold
   different tokens, the outgoing amount is `floor(amount × rate) − fee`, with the fee in the
   **outgoing** leg's unit — the peering ADR 0061 already attached it to. The covering claim
   (`cover_forward`) is minted for that outgoing figure on the outgoing channel, which needs no
   change: claims are already denominated by channel identity alone. The capability is in every
   connector; there is no dealer node class.

2. **Never silently.** A cross-token forward whose ordered pair has no declared rate is
   **refused** with a reject, loudly. There is no implicit 1:1 — an unconverted 18-vs-6-decimals
   pass-through is a 10¹²× error and is the one failure this record exists to make structurally
   impossible. A **same-asset** pair (USDC on Base to USDC on Solana — today's mixed-chain
   topology) has no rate row by definition and forwards unconverted at the flat fee, exactly as
   today. The absence rule is the safety rule: no declared rate, no conversion, no forward.

3. **Rates are declared per token against one numeraire, and composed.** A `[[tokens]]` row
   declares each token the node deals (its chain and contract identity) and optionally its
   **quote**: a path of one or two operator-named AMM pools on that token's **own settlement
   chain** — the one chain the peering already guarantees RPC for — ending at the numeraire.
   One pool for a numeraire-quoted token; two for a token whose only real venue is quoted in an
   intermediate (ANYONE's is a WETH-quoted pool, so its quote is ANYONE/WETH then
   WETH/numeraire — `docs/research/token-pair-price-sources.md`). Each leg is read by **TWAP
   only**, over an operator-set window, and the legs' floors compose. There is no spot read and
   no pool discovery; a pool the operator cannot name does not exist.
   The node declares **one stable numeraire** (USDC, DAI — the operator's choice), and mixing
   numeraires is refused at boot. A cross rate is the composition `X→Y = (X/numeraire) ÷
(Y/numeraire)` with the spread applied on top; a pair that cannot self-source (thin token,
   Solana-side pool, no pool at all) runs a **static rate row** the operator tends by hand, which
   also serves as a per-pair override. Direction is the trade: the spread makes X→Y and Y→X
   different prices from the same mid.

4. **The arithmetic is a rational over base units.** A rate is `numerator/denominator`, both
   `u64`, folding the decimals difference into the ratio; multiplication goes through `u128`; the
   forward rounds **down**, in the connector's favour, always. No floats exist on the money path.
   The `[settlement] decimals` keys remain what they are today: a boot-time assertion against the
   chain, never an input to value arithmetic.

5. **Three guards, per pair with node defaults.** `spread` — the operator's dealing margin,
   applied against the sender; `ttl` — a rate not refreshed within it is dead, and converting
   forwards across that pair refuse until a fresh one lands (a dead poller takes the pair down
   loudly; it never trades on the last price before the outage); `max_move` — a refresh that
   jumps beyond the bound is refused as probable manipulation, the previous value serving out its
   own ttl while a human looks. A flash-loaned pool therefore moves nothing: the TWAP resists it
   within the window and `max_move` refuses what leaks past.

6. **Sourcing is a port; forwarding reads a table.** `RateSource` is its own port with a contract
   suite (ADR 0007), and its first implementation reads **Uniswap-v3-compatible `observe()`
   TWAPs** — the interface Uniswap v3 and Aerodrome Slipstream both carry — over the settlement
   backend's existing RPC endpoint; the backend itself remains no part of any value path. A pool
   without that interface is not a source: **Uniswap v4 core pools have no oracle and are
   excluded** (oracles there are optional per-pool hooks that cannot be assumed present —
   `docs/research/token-pair-price-sources.md`, which also records the per-chain venue defaults
   and the observation-cardinality prerequisite the operator runbook must own). A
   background poller refreshes the rate table; the forwarding path does **no I/O** and only ever
   reads the table. Solana-side sources are out of scope until someone writes that implementation
   against the port; those pairs run static rows. ADR 0009 is not disturbed: the config declares
   the _source_ — pool, window, spread, ttl, bounds — immutably at boot, and the observed rate is
   runtime state on the same footing as a chain balance.

7. **A reject converts its cost back.** As a reject crosses a boundary upstream, the hop adds its
   own fee — in the outgoing unit, where decision 1 puts it — and converts the total into the
   incoming leg's unit, rounding **up**, against itself: the exact inverse of the forward,
   `ceil((cost + fee) / rate)` against `floor(amount × rate) − fee`. The figure therefore arrives at the
   prober denominated in the prober's own unit however many boundaries the path crossed, and a
   prober paying the probed cost always clears (the up/down rounding pair can overstate by a base
   unit, never under). The wire stays unit-silent in both directions: no asset field is added to
   any packet, header, protocolData entry, probe response or vector, because no number ever
   travels in a unit the reader cannot already know. This closes #1286's second gap with no
   change to `vectors/wire-vectors.json`.

8. **The vocabulary gains the words.** **Rate**, **spread** and **segment** enter `CONTEXT.md`;
   **Cost**'s entry gains the sentence that the sum arrives in the asker's own unit; **Fee**'s
   entry keeps the flat-carriage meaning unchanged and its avoid-list is rewritten — "rate" and
   "spread" stop being forbidden words and become these defined ones. A fee buys carriage; a
   spread pays for dealing; they are separate earnings streams and a non-dealing hop still earns
   only the fee. "Arbitrage" stays out of the record: what a dealer earns is a spread, priced
   against risk, not a riskless difference between venues. "Bureau" is retired unused.

## Considered options

**A converting app at a termination (the "bureau").** Designed in full — static catalog of
terminated routes, the app an ordinary toon-client on its far side, gift wrap re-sealed onward.
Zero connector changes, every path single-unit. Rejected by the owner: a second termination and
node per boundary, the catalog mirrored on every reselling edge, the far leg nested inside the
buyer's deadline, and conversion made a product when the topology wants it as plumbing.

**A rate on the peering, beside the fee.** Rejected: a rate is inherently pairwise. Which rate
applies depends on where the packet _arrived_, so a node with three tokens needs ordered pairs,
which a single peering's row cannot express.

**A rate derived from `decimals()`.** Rejected: scale is not price. The decimals gap folds into
the declared ratio; nothing about a market lives on-chain in a token's metadata.

**Spot reads, or pool discovery.** Rejected: a spot ratio is manipulable inside one block, and
letting the connector pick pools invites dust-liquidity decoys. The operator names the pool; the
read is a TWAP; anything else is not a rate.

**An external rate-keeper writing rates through the operator surface.** Viable — it is ADR 0049's
"set from outside" shape and keeps AMM knowledge out of the binary — and rejected by the owner in
favour of self-sourcing: a connector that chose to peer over a token already runs RPC for that
token's chain, and the port boundary (decision 6) quarantines the AMM surface that self-sourcing
costs. The operator-writable rate row remains the natural evolution if a source the port cannot
express ever matters, and nothing in this record obstructs it.

**An environment variable, or mutable rate config.** Rejected on ADR 0009's standing grounds:
there is no env layer, and config is immutable for the process lifetime. What changes at runtime
is an observation, never a declaration.

## Consequences

**The doctrine prose inverts, and the tests follow.** `local_topologies_load.rs`'s mixed-chain
assertions keep their arithmetic — a same-asset chain crossing is _still_ not a conversion, now
by decision 2's absence rule rather than by exile — but the "swap repository's job" prose is
rewritten to cite this record. The RFC 0027 profile's PF-12/PF-13 bullet is rewritten toward the
RFC: a hop applies its local rate, which is ILPv4's own model, policed differently. A dealing
topology belongs under `local/` beside the existing four, proving a USDC-in/ANYONE-out crossing
against real chains.

**`fee.rs` stops being one subtraction.** The converting arm is new arithmetic with new property
tests: round-trip bounds (forward floors, reject-path ceils, the pair never understates a cost),
overflow through `u128`, refusal on the absent pair. `R01`'s "fee alone exceeds the arriving
amount" comparison now happens in the outgoing unit, after conversion.

**An 18-decimal leg has a real ceiling.** Amounts are `u64`, so one packet caps at
`u64::MAX / 10¹⁸ ≈ 18.4` tokens on an 18-decimals leg. The motivating workload sits ~1800× under
it, and `max_packet_amount` — already a per-peer field, already checked against the outgoing
amount — is where an operator expresses it. Worth restating in the peering docs because the cap
is configured in the _outgoing channel's_ unit, which stops being obvious once units differ.

**Staleness is an outage, on purpose.** A pair whose rate expires takes every route across it
down with rejects until the poller recovers or the operator intervenes. That is the
run-or-fail-loudly rule applied to money: the alternative — quietly dealing on the last known
price — is how a dealer is drained politely.

**The dealing operator's book is their own.** This record gives the operator a spread knob and
three guards; it does not manage inventory, rebalance float, or promise the spread covers the
risk. A hop's posted price arithmetic (`price − fee ≥` the next hop's price, converted) remains
the operator's unenforced business, exactly as it is today within one unit.

**What does not change.** Claims stay per-channel and asset-silent; the packet encoding, the
probe mechanism, `Toon-Accumulated-Cost`'s carriage and every existing vector are untouched;
`deny_unknown_fields` and the tombstone discipline stand — the new tables arrive as typed config,
and no removed key is resurrected. A buyer still needs exactly one channel, to its own edge, in
its own unit; if a sender ever must hold the far-side asset or dial the far-side node to cross a
boundary, this design has failed.
