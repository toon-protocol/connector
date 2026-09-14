# Token-pair price sources for `RateSource`

> **Research note, not a decision record.** [ADR 0071](../adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md)
> owns the decisions; this note surveys the sources its `RateSource` port (decision 6) could
> read, against primary sources only — official docs, whitepapers and deployed contract/program
> source. Written 2026-09-10.

ADR 0071 fixes the frame this note evaluates inside: the operator **names** the pool (no
discovery), the read is **TWAP only** (no `slot0`, no spot), the pool lives on the token's **own
settlement chain** (the chain the peering already guarantees RPC for), every token quotes against
**one stable numeraire** and cross rates compose through it, the arithmetic is **integer-rational**
(no floats on the money path), and the `spread` / `ttl` / `max_move` guards (decision 5) are the
backstop for whatever a source cannot itself guarantee. Each source below is scored on one rubric:

- **Trust class** — on-chain-verifiable market read, on-chain attested oracle, or plain API trust.
- **Dependency** — reads a chain the connector already has RPC for, or a new external service.
- **Manipulation resistance** — the mechanism, and what leaks past it for the guards to catch.
- **Long-tail coverage** — can an operator get a price for a thin token like ANYONE.
- **Rust read mechanics** — what the poller actually does over plain RPC.
- **Cost / licensing** — gas, subscription, licence.
- **Failure modes** — what `ttl` / `max_move` must be tuned to catch.

---

## EVM

### 1. Uniswap v3 `observe()` TWAP

**Read mechanics.** Every v3 pool keeps an oracle array of observations; each observation carries a
**tick accumulator** that "grows by the value of the current tick, per second". The caller does not
touch the array directly: [`observe(uint32[] secondsAgos)`](https://docs.uniswap.org/concepts/protocol/oracle)
takes an array of seconds-ago values and returns `int56[] tickCumulatives` (interpolating a
counterfactual observation when no stored one matches the requested time). The TWAP is then

```
arithmeticMeanTick = (tickCumulatives[1] − tickCumulatives[0]) / window
price              = 1.0001^arithmeticMeanTick   (token1 per token0)
```

and, per Uniswap's own docs, "using an arithmetic mean tick to derive a price corresponds to a
_geometric_ mean price" — the standard v3 TWAP is a geometric mean over the window. The reference
consumer is [`OracleLibrary.consult`](https://github.com/Uniswap/v3-periphery/blob/main/contracts/libraries/OracleLibrary.sol)
(`observe([secondsAgo, 0])`, delta divided by window, with an explicit round-toward-negative-infinity
correction) and `getQuoteAtTick` for tick → amount conversion. For an off-chain poller the whole
read is **one `eth_call` of `observe`** plus pure integer math; no state is written.

**No floats needed.** `1.0001^tick` sounds like floating point but is not: v3-core's
[`TickMath.getSqrtRatioAtTick`](https://github.com/Uniswap/v3-core/blob/main/contracts/libraries/TickMath.sol)
computes the Q64.96 fixed-point `sqrtPriceX96` for any tick in pure integer arithmetic. A Rust port
of that function gives an exact `U256` ratio, which the poller then reduces to ADR 0071's
`u64/u64` rational (a bounded rounding step, done off the money path, before the table write).
This satisfies the ADR's no-`f64` falsifier.

**Cardinality — who pays.** A pool is initialized with an oracle array of length **1** (the
observation is overwritten each block, so `observe` over any real window reverts with `OLD`).
"[Anyone] willing to pay the transaction fees may increase the number of tracked observations (up
to a maximum of 65535)" via `increaseObservationCardinalityNext` — a one-time SSTORE cost per slot,
paid by whoever calls it, growing lazily as slots are first used
([oracle concept doc](https://docs.uniswap.org/concepts/protocol/oracle);
[`Oracle.sol`](https://github.com/Uniswap/v3-core/blob/main/contracts/libraries/Oracle.sol) header
comment). **Operational consequence for `RateSource`:** naming a pool is not enough — the operator
must check its cardinality covers the configured window (one `slot0`-free way: call `observe` for
the window and treat the `OLD` revert as "pair not ready") and, if short, pay the one-time gas to
grow it. Observations are only written when swaps touch the pool, so a dead pool's TWAP also goes
stale — the `ttl` guard's case.

**Manipulation cost under proof-of-stake — Uniswap's own guidance.** Uniswap Labs' research post
[Uniswap v3 TWAP Oracles in Proof of Stake](https://blog.uniswap.org/uniswap-v3-oracles)
(Adams/Wan/Zinsmeister, also on [SSRN](https://papers.ssrn.com/sol3/papers.cfm?abstract_id=4384409))
is explicit that the merge weakened the classic defence: a proposer who knows it has **consecutive
blocks** can move the price in the first and restore it in the last, paying no arbitrage toll
in between. Their findings: one-block and two-block manipulations remain prohibitively expensive,
but "three-block and greater attacks are technically feasible", though statistically unlikely for
validators with a small stake share; full-range liquidity depth is the main cost driver, and
Uniswap Labs is researching PoS-resistant designs (the truncated oracle below). **For ADR 0071 this
is the argument for decision 5 being load-bearing:** the TWAP resists a flash loan by construction
(single-transaction moves average out over the window), but multi-block proposer manipulation is
exactly the residual `max_move` exists to refuse.

**Deployments.** Canonical v3 deployments exist on
[Ethereum mainnet, Arbitrum, Optimism, Polygon, Base, BNB, Avalanche and more](https://docs.uniswap.org/contracts/v3/reference/deployments/);
`UniswapV3Factory` is
[`0x1F98431c8aD98523631AE4a59f267346ea31F984` on mainnet](https://docs.uniswap.org/contracts/v3/reference/deployments/ethereum-deployments)
and [`0x33128a8fC17869897dcE68Ed026d694621f6FDfD` on Base](https://docs.uniswap.org/contracts/v3/reference/deployments/base-deployments).
The docs warn integrators to **not** assume same-address across chains.

**Rubric.** On-chain-verifiable market read; zero new dependency (an `eth_call` on the settlement
RPC the peering already guarantees); geometric-mean TWAP manipulation resistance with a known
multi-block PoS hole; long-tail coverage is whatever pool exists (permissionless listing — but see
the ANYONE section); read is one view call + integer math; cost is one-time cardinality gas;
failure modes for the guards: stale pool (no swaps → `ttl`), multi-block manipulation and
liquidity migration away from the named pool (→ `max_move`), `OLD` revert (pair down, loudly).

### 2. Uniswap v4 — no oracle in core; the truncated-oracle hook

First-party sources are unambiguous that **v4 core has no oracle**. The
[v4 vision post](https://blog.uniswap.org/uniswap-v4) and the
[v4 docs architecture](https://docs.uniswap.org/contracts/v4/concepts/v4-vs-v3) state that hooks
make "the protocol-enshrined price oracle that was included in Uniswap v2 and Uniswap v3
unnecessary": v3 enshrined the oracle at the cost of swapper gas, and v4 removes it from core so
it can be implemented as an **optional hook** per pool. The
[v4 whitepaper](https://app.uniswap.org/whitepaper-v4.pdf) records the same removal.

The [truncated oracle hook](https://blog.uniswap.org/uniswap-v4-truncated-oracle-hook) is Uniswap
Labs' PoS answer: a geometric-mean oracle, as a hook, that **caps the recorded tick movement per
block** (threshold 9,116 ticks in the post), so a manipulator "would have to sustain his market
manipulation over time" across many blocks while arbitrage eats the position. Status per the post:
experimental, sample implementation on GitHub, "Uniswap v4 and the truncated hook are still being
developed. The final specs may vary" — not audited, not a deployed standard.

**Consequence: a v4 pool cannot be assumed readable.** Whether a given v4 pool has any oracle at
all — and what its ABI is — depends on which hook the pool creator attached. There is no uniform
`observe()` to target. v4 is therefore **not a v1 dialect** for `RateSource`; the truncated oracle
hook is a plausible _future_ dialect once a canonical audited deployment exists, and its
per-block cap is philosophically the on-chain version of ADR 0071's `max_move`.

### 3. Aerodrome Slipstream on Base — **resolved: `observe()`-compatible**

The open question in ADR 0071's implementation notes resolves cleanly at the source level.
[`aerodrome-finance/slipstream`](https://github.com/aerodrome-finance/slipstream) says in its README
that the core contracts are "adapted from UniswapV3's core contracts" (GPL-2.0-or-later, like
v3-core). Verified directly in the source:

- [`CLPool.sol`](https://github.com/aerodrome-finance/slipstream/blob/main/contracts/core/CLPool.sol)
  carries `Oracle.Observation[65535] public override observations`, implements
  `observe(uint32[] calldata secondsAgos)` returning `(int56[] tickCumulatives, uint160[]
secondsPerLiquidityCumulativeX128s)`, and `increaseObservationCardinalityNext(uint16)` — the
  exact v3 signatures.
- [`ICLPoolDerivedState.sol`](https://github.com/aerodrome-finance/slipstream/blob/main/contracts/core/interfaces/pool/ICLPoolDerivedState.sol)
  is interface-identical to `IUniswapV3PoolDerivedState` for `observe` and
  `snapshotCumulativesInside`.
- Slipstream's [`Oracle.sol`](https://github.com/aerodrome-finance/slipstream/blob/main/contracts/core/libraries/Oracle.sol)
  keeps v3's semantics verbatim, including "Every pool is initialized with an oracle array length
  of 1. Anyone can pay the SSTOREs to increase the maximum length".

So the v3 TWAP dialect — `observe([window, 0])`, delta over window, TickMath — reads an Aerodrome
Slipstream pool **unchanged**, cardinality caveat included. **What remains unverified:** this note
read the `main` branch of the repository, not the bytecode behind any specific pool on Base; before
naming a Slipstream pool, an operator should confirm the pool's verified source on Basescan matches
(standard practice for any named pool, canonical v3 included). Slipstream's gauge staking changes
where LP fees go, not the oracle path.

### 4. Chainlink data feeds (`AggregatorV3Interface`)

**Read mechanics.** A consumer calls
[`latestRoundData()`](https://docs.chain.link/data-feeds/using-data-feeds) on the feed's proxy:
`(uint80 roundId, int256 answer, uint256 startedAt, uint256 updatedAt, uint80 answeredInRound)`,
with `decimals()` (typically 8) scaling the answer. Chainlink's own docs tell consumers to check
`updatedAt` for staleness, and on L2s (Base included) to additionally consult the
[L2 Sequencer Uptime Feed](https://docs.chain.link/data-feeds/l2-sequencer-feeds) — a Base outage
otherwise leaves a fresh-looking stale answer. For the poller: one `eth_call`, integer answer,
trivially rational.

**Cadence and trust.** Per the
[decentralized data model](https://docs.chain.link/architecture-overview/architecture-decentralized-model),
each feed is updated by multiple independent node operators; an on-chain update fires when either a
**deviation threshold** (off-chain price diverges beyond a set percentage) or a **heartbeat**
(maximum time since last update) is hit, whichever first, and the aggregator requires a minimum
number of oracle responses per round. The trust class is an **on-chain attested oracle**: the chain
verifies who signed, not whether the price is true — you trust the DON's operators and Chainlink's
off-chain aggregation, not a market you can audit. Manipulation resistance is therefore
sybil/collusion economics rather than AMM arithmetic; the corresponding failure mode is a wrong or
frozen committee answer, which `max_move` and the heartbeat-vs-`ttl` comparison partially catch.

**Long-tail coverage.** Feeds are provisioned per network by Chainlink and catalogued at
[data.chain.link](https://data.chain.link/); the docs'
[feed-selection guidance](https://docs.chain.link/data-feeds/selecting-data-feeds) grades feeds
into risk categories and warns against low-liquidity assets. **No ANYONE/USD feed appears in the
catalog**, and there is no permissionless path to create one — a new feed is a commercial
provisioning decision by Chainlink, so an ANYONE-shaped long-tail feed is implausible without the
project paying for one. Chainlink fits `RateSource` only for majors (ETH/USD, USDC/USD exist on
Base) — useful, if ever, for the numeraire-sanity leg, not for the thin token that motivates the
dealing feature.

### 5. Other first-party EVM options

**`OracleLibrary` as the reference, not a dependency.** The consult/quote math above is ~40 lines;
the Rust poller should port it (including the negative-delta rounding correction and TickMath)
rather than call any on-chain helper, keeping decision 6's "no I/O on the forwarding path" and the
integer-rational falsifier.

**The truncated oracle hook as a future source** (covered under v4): once audited and canonically
deployed, it would give `RateSource` a venue whose _on-chain_ recording already enforces a
per-block `max_move`, stacking with the connector's own guard. Nothing to build against today.

---

## Solana

### 6. Pyth

**Model, as currently documented.** Pyth is a
[pull oracle](https://docs.pyth.network/price-feeds/core/pull-updates): publishers stream to
Pythnet, the **Hermes** web service serves signed price updates, and "anyone can submit a price
update to the on-chain Pyth contract, which verifies its authenticity and stores it for later use"
— the consumer (or anyone) lands the update in its own transaction and pays the fee. Two
push-shaped conveniences exist on Solana: the
[Push Feeds](https://docs.pyth.network/price-feeds/core/push-feeds) page documents that "the Pyth
Data Association sponsors price updates for a subset of commonly used price feeds" (shard 0), each
at a heartbeat or deviation trigger, and anyone can run the price pusher/scheduler for other feeds
since updating is permissionless.

**The 2026 Pyth Core upgrade changes the economics.** Per
[the Pyth Core upgrade](https://www.pyth.network/blog/the-pyth-core-upgrade) blog and the
[preparation docs](https://docs.pyth.network/price-feeds/core/upgrade/preparing): the new
infrastructure cut over on **July 31, 2026** (the blog's date; the docs state existing on-chain
integrations were auto-upgraded by the DAO on **August 26, 2026**), and **Hermes now requires an
API key** (`https://pyth.dourolabs.app/hermes/`, `Authorization: Bearer`), with a paid model —
Starter at **$500/month** for crypto assets, professional tiers $2,500–$10,000/month. Reading the
**sponsored on-chain push accounts over plain RPC needs no key**; _pulling_ fresh updates yourself
now does. "Free self-serve Pyth pull" is no longer an accurate description.

**Rust read over plain RPC.** The
[`pyth-solana-receiver-sdk`](https://docs.rs/pyth-solana-receiver-sdk) crate defines
[`PriceUpdateV2`](https://docs.rs/pyth-solana-receiver-sdk/latest/pyth_solana_receiver_sdk/price_update/struct.PriceUpdateV2.html):
`write_authority`, `verification_level` (how many Wormhole guardian signatures were verified),
`price_message` (`price: i64`, `conf: u64`, `exponent: i32`, `publish_time: i64`), `posted_slot`.
An off-chain poller does `getAccountInfo` on the feed account and deserializes; the safe accessor
`get_price_no_older_than(&clock, max_age, &feed_id)` enforces staleness against the clock and
requires full verification (the docs explicitly warn `get_price_unchecked` "does not check how
recent the price is"). The **confidence interval** `conf` is Pyth's published uncertainty
(price ± conf, per the [best practices](https://docs.pyth.network/price-feeds/core/best-practices)
doc, which recommends rejecting or widening on large `conf`) — it maps naturally onto `max_move`
tuning, and `publish_time` maps onto `ttl`.

**Long-tail coverage.** Feeds require first-party publishers (exchanges, market makers) and are
requested [via Discord](https://docs.pyth.network/price-feeds); the catalog is queryable at
Hermes `GET /v2/price_feeds?query=…`. Queried directly for this note: `query=anyone` returns
`[]` while a control `query=bonk` returns feeds — **no ANYONE feed exists**, and a thin token with
no professional market makers is unlikely to get one.

### 7. Switchboard

Switchboard's current product is [On-Demand](https://docs.switchboard.xyz/) — a pull model where
"feeds are created and used only when needed", and feed creation is explicitly permissionless:
"Deploy your new data feed, stream from any source on or off-chain, and set the exact parameters
that you need. No need to wait for contracts or red tape." A feed definition names its sources
(CEX APIs, DEX pools, arbitrary jobs); oracles on a queue execute it and sign results; the
consumer lands the update in its own transaction (and pays that fee). On-chain the
[`switchboard-on-demand`](https://crates.io/crates/switchboard-on-demand) crate reads a
[`PullFeedAccountData`](https://switchboard-on-demand-rust-docs.web.app/on_demand/accounts/pull_feed/struct.PullFeedAccountData.html)
account — fields include `submissions`, `max_variance`, `min_responses`, `feed_hash` — with
`get_value(clock, max_stale_slots, min_samples, only_positive)` returning the **median** of recent
oracle submissions and erroring on staleness or too few samples: staleness and dispersion checks
are built into the read.

**Rubric.** Attested-oracle trust class (you trust the sources the feed definition names plus the
oracle queue — for a long-tail token the definition would in practice wrap the very DEX pool a
direct read would target, adding a trust layer rather than removing one). Long-tail coverage is the
best of the oracle options _because_ creation is permissionless, but someone (the operator) must
create the feed, fund its updates, and keep the crank running — a new operational dependency and
per-update Solana fees. Plausible as a port implementation an operator opts into; not the default.

### 8. Orca Whirlpools and Raydium CLMM — pool-native oracles

**Orca: no usable price oracle.** In the Whirlpool program source, the swap instruction takes an
`oracle` PDA (`seeds = [b"oracle", whirlpool]`) annotated
"[CHECK: Oracle is currently unused and will be enabled on subsequent updates](https://github.com/orca-so/whirlpools/blob/main/programs/whirlpool/src/instructions/swap.rs)".
The oracle account that _is_ live holds
[`AdaptiveFeeConstants` / `AdaptiveFeeVariables`](https://github.com/orca-so/whirlpools/blob/main/programs/whirlpool/src/state/oracle.rs)
— a volatility accumulator driving the adaptive fee rate — **not** tick-cumulative price history.
There is no TWAP to read from a Whirlpool without an external service.

**Raydium CLMM: a real v3-style on-chain TWAP.** The CLMM program's
[`oracle.rs`](https://github.com/raydium-io/raydium-clmm/blob/master/programs/amm/src/states/oracle.rs)
defines an `ObservationState` PDA per pool (seed `"observation"`, linked from
`pool_state.observation_key`): a ring of `OBSERVATION_NUM = 100` observations, each
`{ block_timestamp: u32, tick_cumulative: i64 }`, written at most once per
`OBSERVATION_UPDATE_DURATION_DEFAULT = 15` seconds — and the
[swap instruction](https://github.com/raydium-io/raydium-clmm/blob/master/programs/amm/src/instructions/swap.rs)
takes `observation_state` as a mutable account, i.e. observations are written **during swaps**.
That is the same tick-cumulative arithmetic as Uniswap v3, with two structural differences: the
history is a **fixed 100 slots** (≈25 minutes at the 15s cadence — no cardinality to grow, but
also no longer window), and there is no `observe()` view — an off-chain poller does
`getAccountInfo` on the observation account, deserializes the struct (the program crate's state
types, or a hand-rolled zero-copy layout), picks two observations spanning the window and computes
`Δtick_cumulative / Δt` itself. Because observations land only on swaps, a quiet pool's newest
observation ages — on-chain staleness the `ttl` guard must read off `block_timestamp`, not off the
poll succeeding.

**Rubric (Raydium).** On-chain-verifiable market read over RPC the Solana settlement backend
already has — the only Solana option in the same trust class as the EVM v3 read; same math dialect
(tick cumulative → integer tick → price rational); manipulation resistance is the TWAP window
(≤ ~25 min) with **no** PoS-proposer literature equivalent — Solana leaders also order blocks, so
`max_move` stays load-bearing; long-tail coverage is whatever CLMM pool exists; cost is nil beyond
RPC. This is the natural first Solana implementation of the `RateSource` port.

### 9. Jupiter Price API v3

[Jupiter Price API v3](https://dev.jup.ag/docs/price-api/v3) is an **HTTPS API**
(`https://api.jup.ag/price/v3`, up to 50 mints per call) returning `usdPrice`, `blockId`,
`decimals` and `priceChange24h` per token. The price is "the last swapped price (across all
transactions)", derived by working outward from a small set of reliable tokens (like SOL) "whose
price we get from external oracle sources", filtered through heuristics (liquidity, holder
distribution, trading activity, Organic Score); a token failing them, or unswapped for ~7 days,
is **omitted entirely** from the response. [Rate limits](https://dev.jup.ag/portal/rate-limit) are
plan-tiered — a free tier (`lite-api.jup.ag`, 60-second window) and paid API-key tiers
(`api.jup.ag`) with 429s past the window.

**Rubric.** Pure API trust — nothing on-chain verifies the number; availability is Jupiter's
infrastructure; the last-swap derivation means one manipulated swap in a thin pool _is_ the price
until the heuristics catch it. Coverage of Solana-traded tokens is the broadest of any option here.
Against ADR 0071's frame this is not a `RateSource` implementation candidate (the ADR's rejected
"external rate-keeper" is the honest home for it: an operator script consulting Jupiter and
tending static rows / a future operator-writable rate row); it is however a fine _comparison_
input for a human tuning `max_move`.

---

## The motivating long-tail token: ANYONE

- ANYONE is an **ERC-20 on Ethereum mainnet**,
  [`0xFeAc2Eae96899709a43E252B6B92971D32F9C0F9`](https://etherscan.io/token/0xFeAc2Eae96899709a43E252B6B92971D32F9C0F9),
  fixed supply 100M ([anyone.io/token](https://www.anyone.io/token)). It is not a Solana token.
- Its principal venue is a **Uniswap v3 ANYONE/WETH 1% pool on mainnet**
  ([`0xc593…30cc`](https://etherscan.io/address/0xc593fe9193b745447e86b45ea0bf62565ee030cc); the
  ~$3M liquidity figure comes from GeckoTerminal, a third-party aggregator — the pool itself and
  its fee tier are verifiable on-chain).
- **No oracle covers it:** no Chainlink feed in the catalog, Pyth's catalog returns nothing for it,
  and Jupiter cannot see it (wrong chain).

Two frictions with ADR 0071's decision-3 shape follow, reported as findings rather than resolved:

1. **The numeraire pool may not exist.** ANYONE quotes against WETH, not USDC/DAI; a direct
   ANYONE/_stable_ pool is not in evidence. Self-sourcing ANYONE/USDC therefore needs a **chained
   read** — TWAP(ANYONE/WETH) × TWAP(WETH/USDC) — i.e. the `[[tokens]]` quote would have to name a
   pool _path_, not one pool, or ANYONE runs a static row despite an honest on-chain venue
   existing. This is the single biggest gap between the ADR's per-token-vs-numeraire model and
   where thin-token liquidity actually sits (WETH-quoted pools dominate the long tail).
2. **The venue is on a chain the connector does not settle on.** ANYONE's liquidity is on Ethereum
   mainnet; the connector's EVM contracts have never been deployed to mainnet (ADR 0056 /
   production-empty), and devnet settles Base Sepolia. Decision 3's "the token's own settlement
   chain" premise — RPC already guaranteed — holds only once an ANYONE peering actually settles on
   the chain the pool lives on. Until then ANYONE is exactly the "pair that cannot self-source"
   case and runs a **static rate row**, which the ADR already provides for.

---

## Summary rubric

| Source                 | Trust class                        | New dependency                                                         | Manipulation resistance                                | Long tail (ANYONE-class)                             | Rust read                                                  | Cost                      |
| ---------------------- | ---------------------------------- | ---------------------------------------------------------------------- | ------------------------------------------------------ | ---------------------------------------------------- | ---------------------------------------------------------- | ------------------------- |
| Uniswap v3 `observe()` | on-chain market read               | none (settlement RPC)                                                  | geometric-mean TWAP; multi-block PoS hole → `max_move` | any pool that exists; ANYONE: WETH-quoted, mainnet   | 1 `eth_call` + TickMath port                               | one-time cardinality gas  |
| Uniswap v4             | n/a — no core oracle               | hook-dependent                                                         | truncated hook (experimental)                          | not assumable                                        | none uniform                                               | n/a                       |
| Aerodrome Slipstream   | on-chain market read               | none                                                                   | identical to v3                                        | Base pools only                                      | same dialect as v3                                         | same as v3                |
| Chainlink              | attested oracle (DON)              | none (on-chain read)                                                   | committee economics; deviation+heartbeat               | **no** — provisioned, no ANYONE feed                 | 1 `eth_call`                                               | free to read              |
| Pyth                   | attested oracle                    | Hermes for pulls (API key, $500+/mo); none for sponsored push accounts | publisher aggregation + `conf`                         | **no** — needs first-party publishers; catalog: none | `getAccountInfo` + receiver SDK                            | free read / paid pull     |
| Switchboard            | attested oracle (operator-defined) | crank + update fees                                                    | median, `max_variance`, `min_responses`                | yes, if operator builds & runs the feed              | `getAccountInfo` + on-demand crate                         | Solana tx fees per update |
| Orca Whirlpools        | —                                  | —                                                                      | —                                                      | —                                                    | **no price oracle** (unused stub; adaptive-fee state only) | —                         |
| Raydium CLMM           | on-chain market read               | none (settlement RPC)                                                  | tick-cumulative TWAP, ≤ ~25 min window                 | any CLMM pool that exists                            | `getAccountInfo` + struct parse                            | none                      |
| Jupiter Price v3       | API trust                          | HTTPS + rate limits                                                    | heuristics, last-swap based                            | broad on Solana; ANYONE: wrong chain                 | HTTP client                                                | free tier / paid keys     |

## Recommendation (consistent with ADR 0071)

- **v1 EVM dialect: the Uniswap v3 `observe()` dialect.** One `eth_call` of
  `observe([window, 0])`, tick-cumulative delta over the window, a Rust TickMath port to a
  `u64/u64` rational — no floats, no `slot0`, no on-chain helper. That single dialect covers
  canonical v3 on mainnet and Base **and** Aerodrome Slipstream on Base unchanged. The poller must
  treat the `OLD` revert as "pair not ready" and the operator runbook must cover the one-time
  `increaseObservationCardinalityNext` cost for a freshly named pool.
- **The Base venue question is resolved to an operator choice, not a dialect choice.** Slipstream
  is `observe()`-compatible at the source level (verified above), so canonical v3 vs Aerodrome on
  Base differ only in where liquidity is deeper for the operator's pair — the ADR's operator-names-
  the-pool rule already carries this; the only residual diligence is diffing the named pool's
  verified source on Basescan.
- **Solana default candidate, when self-sourcing lands: Raydium CLMM `ObservationState`.** It is
  the only pool-native on-chain TWAP found on Solana (Orca's oracle is an unused stub), it is the
  same tick-cumulative math as the EVM dialect, and it reads over the Solana RPC the settlement
  backend already has. Its fixed ~25-minute maximum window and swap-driven observation writes make
  the `ttl` guard non-optional. Pyth's sponsored push accounts are the fallback for majors
  (key-free RPC read, but attested-oracle trust and post-upgrade paid pulls); Switchboard is the
  opt-in for an operator willing to run a feed. Until then, per decision 6, Solana pairs run
  static rows.
- **What stays the operator's:** naming the pool (or accepting none and tending a static row), the
  window, the numeraire, and the three guards. ANYONE itself, today, is a static-row token — its
  only honest venue is WETH-quoted on a chain the connector does not yet settle on — and the one
  design question this research kicks back to a future record is whether a `[[tokens]]` quote may
  name a **two-pool chain** through WETH, since that is where the long tail's liquidity actually
  lives.
