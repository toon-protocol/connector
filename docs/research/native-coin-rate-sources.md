# Rate sources for a native coin: XRP on XRPL, and MINA

> **Research note, not a decision record.** It decides nothing. The question comes from issue
> [#1567](https://github.com/toon-protocol/connector/issues/1567) (map
> [#1563](https://github.com/toon-protocol/connector/issues/1563)). On 2026-10-28 the owner decided
> that a native-coin-only chain, such as XRPL whose channels hold XRP only, is admitted as a peering
> leg under ordinary [ADR 0071](../adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md)
> dealing. Its asset is `xrpl:native` inside the connector, and in the greeting it is CAIP-19
> `slip44:144` on `xrpl:0`, following
> [ADR 0080](../adr/0080-a-client-may-pay-from-a-zk-note.md) decision 7. This note asks where that
> leg's rate can come from. Primary sources only. Written 2026-10-28, and every source was read
> that day.

**Evidence tags.** CONFIRMED-CODE means read in source at a pinned commit, or read live from a
chain's RPC. CONFIRMED-DOCS means read in the maintainer's own documents or specs. UNCONFIRMED
means none of those, and says why. A sentence marked **Inference** is this note's reasoning from
confirmed facts, not a fact.

**Sources pinned.**

- **rippled / xrpld**: [`XRPLF/rippled@05847a3c`](https://github.com/XRPLF/rippled/tree/05847a3c908a9b3eaf43ddbd9af3c81214e60268)
  (`develop`, 2026-10-27). Cited as **RIPPLED**.
- **XLS standards**: [`XRPLF/XRPL-Standards@8a6427c0`](https://github.com/XRPLF/XRPL-Standards/tree/8a6427c0da1fcbaefa7666ea14f3ef85151e2620)
  (`master`, 2026-10-27): `XLS-0030-automated-market-maker/README.md` (**XLS-30**) and
  `XLS-0047-PriceOracles/README.md` (**XLS-47**).
- **xrpl-rust**: [`XRPLF/xrpl-rust@2a5191b1`](https://github.com/XRPLF/xrpl-rust/tree/2a5191b1),
  the same pin as [`x402-pending-chain-bindings.md`](x402-pending-chain-bindings.md) (cited as
  **PENDING-NOTE**).
- **Live mainnet reads**: JSON-RPC to `s1.ripple.com:51234` (a Clio server, full history
  `32570-107960093`) and `s2.ripple.com:51234`, validated ledgers **107960015–107960093**, about
  14:20–14:30 UTC on 2026-10-28.
- **Issuers**: Circle's [USDC contract addresses](https://developers.circle.com/stablecoins/usdc-contract-addresses)
  page (XRPL mainnet `5553444300000000000000000000000000000000.rGm7WCVp9gb4jZHWTEtGUr4dd74z2XuWhE`).
  Ripple's [RLUSD token addresses](https://docs.ripple.com/products/stablecoin/overview/token-addresses)
  and [RLUSD on the XRPL](https://docs.ripple.com/stablecoin/developer-resources/rlusd-on-the-xrpl)
  pages give the issuer `rMxCKbEDwqr76QuheSUMdEGf4B9xJ8m5De`; the RLUSD page was read through a
  search summary because the direct fetch returned 404. Both issuers then answered `amm_info` live
  (§1a).
- **This repository**: `HEAD` `9a039468` (branch `adr-0077-zk-note`).

---

## Verdict

**XRPL has no pool-native TWAP.** An XLS-30 AMM keeps only its current balances. The order book is
spot. Nothing on the ledger accumulates price over time the way Uniswap v3's `observe()` does. So
no XRPL source can meet ADR 0071 decision 3 ("read by **TWAP only** … There is no spot read") **as
written**.

**XRPL does have a live, native oracle primitive (XLS-47), and one publisher on it quotes exactly
what the leg needs.** DIA's document on mainnet carries XRP/USD, RLUSD/USD and USDC/USD at 8
decimals, updated about every 0.5–6 h in the last 4 days. Reading it is an attested-oracle read,
the same trust class as Chainlink in
[`token-pair-price-sources.md`](token-pair-price-sources.md) §4, not a market read. ADR 0071 does
not admit that class today, so this option needs an amendment.

**The deep XRP/stablecoin liquidity is real.** XRP/RLUSD holds about **$4.8 M** and XRP/USDC about
**$0.27 M** (ledger 107960022). A ledger-sampled time-weighted mean over historical `amm_info`
reads is mechanically possible, but it is a TWAP the connector computes, not one the pool keeps.

**Whatever the source, the config must change.** `AssetChain` is a closed `{Evm, Solana}` enum.
Every peering must resolve to a token declared in `[[tokens]]`, and both sides of every `[[rates]]`
row must be declared tokens. So an XRPL peering cannot copy ADR 0080's MINA rule that "`[[tokens]]`
still may not" name the native asset. `xrpl:native` **must** be a `[[tokens]]` row (§2).

| #   | Question                                               | Answer                                                                                                                                                                                        | Tag                                    |
| --- | ------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------- |
| 1a  | Does an XLS-30 AMM expose TWAP or observation history? | No. Spot price is "the weighted ratio of the instance's pool balances"; no accumulator, no observation array                                                                                  | CONFIRMED-DOCS                         |
| 1a  | AMM amendment live on mainnet?                         | Yes. `AMM` is enabled, and RIPPLED now lists it under `XRPL_RETIRE_FEATURE(AMM)`. `fixAMMv1_1`, `fixAMMv1_3` and `AMMClawback` are enabled too                                                | CONFIRMED-CODE (RPC, src)              |
| 1a  | XRP/RLUSD pool                                         | `rhWTXC2m2gGGA9WozUaoMm6kLAVPb1tcS3`: 1,826,667.8 XRP / 2,398,976.6 RLUSD (≈1.3133 RLUSD per XRP), fee 0.214%                                                                                 | CONFIRMED-CODE (RPC)                   |
| 1a  | XRP/USDC pool                                          | `rM7cHVPfhe9yxQNk2kDNBEQqoQmMcQGPWE`: 102,229.2 XRP / 134,189.1 USDC (≈1.3126), fee 0.097%                                                                                                    | CONFIRMED-CODE (RPC)                   |
| 1b  | PriceOracle (XLS-47) live on mainnet?                  | Yes. `PriceOracle` and `fixPriceOracleOrder` are both enabled                                                                                                                                 | CONFIRMED-CODE (RPC)                   |
| 1b  | Who publishes?                                         | DIA: `rP24Lp7bcUHvEW7T7c8xkxtQKKd9fZyra7`, document 42, provider `diadata`, with XRP/USD, RLUSD/USD, USDC/USD, USDT/USD, BTC, ETH and EUROP. Band's native account was not found              | CONFIRMED-CODE (RPC); Band UNCONFIRMED |
| 1b  | Does `get_aggregate_price` give a time window?         | **No.** It returns mean, median and an optional trimmed mean **across oracles**, one price per oracle. `time_threshold` drops oracles older than the newest one; it is not a window over time | CONFIRMED-CODE                         |
| 1b  | Fits ADR 0071 TWAP-only?                               | No. An oracle is neither a pool nor a TWAP. It needs an amendment that admits an attested-oracle source class                                                                                 | Inference                              |
| 1c  | Does the order book have a time-weighted primitive?    | No. `book_offers` is the current book. AMM and book liquidity are routed together, but neither keeps history                                                                                  | CONFIRMED-DOCS                         |
| 1d  | Disposable per-test chain with AMM and Oracle?         | Yes: stand-alone `xrpld -a --start`. AMM is retired, so it is always on. `PriceOracle` is `DefaultNo`, so it must be listed in the config's `[amendments]` before the genesis ledger          | CONFIRMED-CODE                         |
| 2   | Does code forbid a native asset in `[[tokens]]`?       | Not by name. `xrpl:` and `mina:` are refused as an **unknown chain**, and `evm:native` would parse as a token literally named `native`                                                        | CONFIRMED-CODE                         |
| 3   | Any on-chain MINA oracle or TWAP?                      | None usable. There is no stablecoin on Mina L1, so there is no MINA/stable pool to read                                                                                                       | CONFIRMED-DOCS (prior note)            |

---

## 1. Reading an XRP price on XRPL

### 1a. XLS-30 AMM: spot only

**Mechanism.** XLS-30 §1 defines a weighted geometric-mean market maker. Its "**Spot-Price**: Spot-price (SP) is the weighted
ratio of the instance's pool balances" (XLS-30 L59). It has no accumulator, no observation array
and no time field. The xrpl.org [AMM concept page](https://xrpl.org/docs/concepts/tokens/decentralized-exchange/automated-market-makers)
says the same: a 0.5-weight geometric mean whose rate moves with balances. Its fee is voted by LPs
from 0–1%, and a 24 h **auction slot** sells a one-tenth fee to arbitrageurs. CONFIRMED-DOCS. The
`amm_info` result carries `amount`, `amount2`, `lp_token`, `trading_fee`, `vote_slots` and
`auction_slot`, and no price history. CONFIRMED-CODE (RPC).

**Manipulation properties the spec claims.** XLS-30 argues that low fees and fast finality narrow
the arbitrage window. It also says "Transactions on XRPL are canonically ordered … XRPL doesn't
have" front-running or reordering by block producers (L32–36). CONFIRMED-DOCS. **Inference:** this
removes miner-extractable value, not manipulation by a funded actor. A single `AMMDeposit` or
`Payment` still moves the spot price for at least one validated ledger, about 3–5 s
(PENDING-NOTE §2.6). XRPL has no contract layer, so a borrow, move and repay cannot happen
atomically inside one transaction the way an EVM flash loan does. A manipulator must therefore
hold real inventory across a ledger close and be arbitraged against.

**Live state** (ledger 107960022, `s1.ripple.com`):

| Pool       | AMM account                          | Reserves                                      | Implied price      | Trading fee  | ≈ TVL   |
| ---------- | ------------------------------------ | --------------------------------------------- | ------------------ | ------------ | ------- |
| XRP/RLUSD  | `rhWTXC2m2gGGA9WozUaoMm6kLAVPb1tcS3` | 1,826,667.813272 XRP / 2,398,976.629509 RLUSD | 1.3133 RLUSD / XRP | 214 (0.214%) | $4.8 M  |
| XRP/USDC   | `rM7cHVPfhe9yxQNk2kDNBEQqoQmMcQGPWE` | 102,229.176604 XRP / 134,189.057565 USDC      | 1.3126 USDC / XRP  | 97 (0.097%)  | $0.27 M |
| RLUSD/USDC | `rfWfhoLj3LiaBpoxus3h7pEF8jXDMtyGqb` | 25,104.83 RLUSD / 25,087.61 USDC              | ≈1.0007            | 144          | $0.05 M |

CONFIRMED-CODE (RPC). The fee unit is 1/100,000, so 1000 means 1%, per the
[AMM concept page](https://xrpl.org/docs/concepts/tokens/decentralized-exchange/automated-market-makers).
TVL is twice the stablecoin side. **Inference:** on a constant-product pool, raising the price
10% takes swapping in roughly 4.9% of the quote reserve. That is about $117 k of RLUSD on
XRP/RLUSD, and only about $6.5 k on XRP/USDC.

**Amendments.** The `feature` RPC on mainnet returned `"enabled": true` for `AMM`, `fixAMMv1_1`,
`fixAMMv1_3` and `AMMClawback` (ledger 107960015). CONFIRMED-CODE (RPC). In RIPPLED,
`include/xrpl/protocol/detail/features.macro` lists `XRPL_RETIRE_FEATURE(AMM)` (L125). A retired
amendment is unconditionally on. CONFIRMED-CODE.

**A TWAP the connector would compute.** Every rippled method takes a ledger specifier, so a poller
can read `amm_info` at past ledgers. `s2.ripple.com` answered for ledgers 107959000, 107900000 and
107000000: XRP/RLUSD reserves were 1,825,672.6 / 2,400,258.1, then 1,834,783.9 / 2,385,746.7, then
1,663,116.9 / 2,331,233.3. CONFIRMED-CODE (RPC). **Inference:**

- A ledger-sampled time-weighted mean is possible: read N ledgers across the window and weight
  each by its close time. It still has costs.
  - It needs an RPC endpoint that **keeps** the window's history. A node with `online_delete`
    keeps only a configurable tail; that limit was not checked here.
  - It costs N reads per refresh.
  - Its manipulation resistance is sampling, not accumulation. An attacker must hold the price
    off-market at the sampled ledgers. Uniswap v3's accumulator, by contrast, counts every second.
- This is a TWAP in effect, not in kind. ADR 0071 decision 6 names the Uniswap-v3-compatible
  `observe()` dialect as the first implementation and says "a pool without that interface is not
  a source". Whether that sentence binds every implementation or only the first is an owner reading.

### 1b. XLS-47 Price Oracles and `get_aggregate_price`

**Object.** A `PriceOracle` ledger entry holds `Owner`, `Provider`, `AssetClass`,
`LastUpdateTime` (Unix seconds) and a `PriceDataSeries` of up to ten `PriceData`
(`BaseAsset`, `QuoteAsset`, `AssetPrice` as a `UINT64`, and `Scale`). XLS-47 L27–61.
`OracleSet` rejects a `LastUpdateTime` that is earlier than the previous one, or "not in the range
of the last close time minus/plus 300 seconds" (L174). The provider "has to publish the `Owner`
account public key" and a registry of its document IDs (L108–109). CONFIRMED-DOCS. **The ledger
keeps only the current object.** History exists only as the transaction metadata chain
(`PreviousTxnID`).

**`get_aggregate_price` statistics: across oracles, not across time.** RIPPLED
`src/xrpld/rpc/handlers/orderbook/GetAggregatePrice.cpp`:

- It accepts 1–200 oracles (`kMaxOracles = 200`, L159), an optional `trim` of 1–25%, and an
  optional `time_threshold` in seconds.
- For each oracle, `iteratePriceData` reads the current object. Only when the pair has no
  `AssetPrice` there does it walk back through **up to three** earlier metadata versions
  (`kMaxHistory = 3`, L57). It stops at the first version that has the pair, so **each oracle
  contributes exactly one price**.
- `time_threshold` keeps the prices whose `LastUpdateTime` falls in
  `[latest − threshold, latest]`, where `latest` is the **newest oracle's** time (L313–337). It is
  never compared with the ledger's close time or with now.
- It returns `entire_set {mean, size, standard_deviation}`, `median`, `trimmed_set` when `trim`
  is set, and `time` (the newest `LastUpdateTime`). XLS-47 L372 describes the same selection rule.

CONFIRMED-CODE, CONFIRMED-DOCS. The xrpl.org reference page names the field `trim_threshold`, but
the source reads `jss::time_threshold`, as XLS-47 does. **Treat the doc page as wrong.**
**Inference:** the caller must check staleness itself, by comparing `time` with its own clock.
The ADR 0071 `ttl` guard already measures age from the observation time, which fits that check.

**Live on mainnet.** `PriceOracle` and `fixPriceOracleOrder` return `"enabled": true` at ledger 107960015. CONFIRMED-CODE (RPC). In RIPPLED, `PriceOracle` is `Supported::Yes, VoteBehavior::DefaultNo`
(features.macro L66). That flag matters only for a fresh chain (§1d).

**Who publishes.**

- **DIA.** The XRPL blog post [Integrating DIA Oracles on the XRP Ledger](https://xrpl.org/blog/2025/integrating-dia-oracles-on-xrpl)
  names account `rP24Lp7bcUHvEW7T7c8xkxtQKKd9fZyra7` and document 42. CONFIRMED-DOCS. A live
  `ledger_entry` read of it at ledger 107960075 returned:
  - `Provider` = `diadata` and `AssetClass` = `currency`;
  - `LastUpdateTime` 1793191022, which is 2026-10-28 12:37:02 UTC, about 1 h 50 min before the
    read;
  - XRP/USD `1.32242114`, RLUSD/USD `0.99992908`, USDC/USD `0.99965674`, USDT/USD `0.99905895`,
    EUROP/USD `1.11368954`, plus BTC/USD and ETH/USD, all at `Scale` 8;
  - stale XRP, BTC and ETH entries with no `AssetPrice`.

  The last 30 `OracleSet` transactions (`account_tx`) run from 2026-10-24 23:36 to 2026-10-28
  12:37 UTC, with gaps from about 1 min to about **5 h 40 min**. That pattern looks like
  deviation-triggered updates with a long heartbeat. CONFIRMED-CODE (RPC). DIA's own methodology
  page is behind a Cloudflare challenge and was not read. Its update rule and sources are therefore
  UNCONFIRMED.

- **Band.** Band's blog [announces](https://blog.bandprotocol.com/bandprotocol-launches-on-xrpl-mainnet/)
  XRP/USD, BTC/USD and ETH/USD "on the XRP Ledger (XRPL) Mainnet". CONFIRMED-DOCS (vendor). No
  publisher account or document ID was found, and a full `ledger_data` scan for `oracle` objects
  was abandoned after 400 pages. **UNCONFIRMED** whether Band publishes through XLS-47 on mainnet.
- **Chainlink** appears only as an example `Provider` string in XLS-47 (L45). No evidence was
  found that it publishes on XRPL. UNCONFIRMED.
- **Not XLS-47, but live.** XRPL Labs' aggregated XRP/USD, from Wietse Wind's
  [post](https://dev.to/wietse/aggregated-xrp-usd-price-info-on-the-xrp-ledger-1087) of 2021, is
  the `LimitAmount` of a `TrustSet` from `rXUMMaPpZqPutoRszR29jtC8amWq3APkx`. It takes the median
  of six exchanges, drops outliers beyond 1σ, and updates every minute. It is still running: its
  last three `TrustSet`s were 60 s apart, the newest at 14:27 UTC with value `1.3104`.
  CONFIRMED-CODE (RPC). It is a single-publisher feed and is not readable by
  `get_aggregate_price`.

**Arithmetic fit.** `AssetPrice` is a `u64` and `Scale` is a `u8`, so a price is exactly
`AssetPrice / 10^Scale`. That is ADR 0071 decision 4's integer-rational shape, with no float.
The aggregate `mean` is an `STAmount` decimal string; it parses to a rational, but trimming happens
in rippled. **Inference.**

**Numeraire fit.** XLS-47 quotes against a **currency code** (`USD`), not an issued token. DIA's
USDC/USD row lets a source compose XRP→USDC as `(XRP/USD) ÷ (USDC/USD)`. That is the same
two-leg shape as ADR 0071's ANYONE/WETH→WETH/USDC path, with "USD" as the intermediate.
**Inference.**

**Fit with ADR 0071.** The record says "Each leg is read by **TWAP only**, over an operator-set
window" (decision 3), and its first implementation "reads Uniswap-v3-compatible `observe()`
TWAPs" (decision 6). An XLS-47 read is neither a pool nor a window. The research note behind ADR
0071 already sorts this kind of source into a separate trust class, "on-chain attested oracle: the
chain verifies who signed, not whether the price is true"
([`token-pair-price-sources.md`](token-pair-price-sources.md) §4). It scored Chainlink and Pyth "for
majors" and did not adopt them. **An XLS-47 `RateSource` therefore needs an amendment to ADR
0071**, not just an implementation, because it adds a source class. The port contract also has to
be reread. Promise 2 says "a window shorter than the source can serve is an **error**". For an
oracle, "window" can only mean the maximum age of `LastUpdateTime`, which is a different promise
(`crates/connector-rate-source/src/contract.rs` L183–216). **Inference.**

### 1c. The central order book

`book_offers` returns the offers on one side of a book at a ledger. The DEX routes payments
"through pools, order books, or both, whichever gives the better rate"
([AMM concept page](https://xrpl.org/docs/concepts/tokens/decentralized-exchange/automated-market-makers);
XLS-30 L24). CONFIRMED-DOCS. No time-weighted primitive exists. The only way to time-weight the book
is the same ledger-sampling as §1a. It is worse here, because an offer can be placed and
cancelled within one ledger without trading. **Inference.**

### 1d. A disposable chain per test

PENDING-NOTE §2.5 already establishes that stand-alone `xrpld -a --start` runs from a genesis
ledger with `ledger_accept` and no clock warp, in Docker as `rippleci/xrpld:<tag>`. What it adds
for a rate source:

- **AMM is always on.** It is a retired feature (§1a). A test funds two accounts from the genesis
  master, issues an IOU and calls `AMMCreate`. CONFIRMED-CODE (retirement). The remaining steps are
  **Inference**.
- **PriceOracle is not on by default.** `startGenesisLedger` seeds the genesis with
  `amendmentTable_->getDesired()` (RIPPLED `src/xrpld/app/main/Application.cpp` L1690–1694). That
  list holds only `supported && vote == Up` amendments (`AmendmentTable.cpp` L783–807), and
  `DefaultNo` maps to `AmendmentVote::Down` (L551–553). The config's `[amendments]` section
  persists a vote of `Up` (L567–580). So the test's `xrpld.cfg` must list `PriceOracle` by hash
  and name. CONFIRMED-CODE.
- **`OracleSet` from a test account works within the ±300 s close-time rule.** Stand-alone closes
  ledgers when asked, at wall-clock time. **Inference.**
- **A ledger-sampled AMM mean (§1a) would need real seconds between ledgers**, because there is no
  clock warp. That is the same cost PENDING-NOTE D7 records for `SettleDelay`. **Inference.**
- **xrpl-rust at the pin has request and result models** for `amm_info`, `get_aggregate_price`,
  `ledger_entry` and `book_offers`, plus the `oracle` ledger object and `OracleDelete`.
  CONFIRMED-CODE (tree listing).

So a `RateSource` contract suite for either XRPL source can follow CLAUDE.md's chain-per-test rule,
under the same Linux/Docker-only caveat PENDING-NOTE §2.5 raises.

---

## 2. What the connector's code says about a native asset

All at `HEAD` `9a039468`. CONFIRMED-CODE.

- **`AssetChain` is closed.** `crates/connector-domain/src/asset.rs` L77–82 has the variants `Evm` and
  `Solana`. `FromStr` maps any other prefix to `AssetIdError::UnknownChain` (L100–110). The test
  `an_unknown_chain_is_refused_by_name` asserts that `"mina:B62qFoo"` is refused as `UnknownChain("mina")`
  (L314–319). **No code refuses "native" as such.** `mina:native` and `xrpl:native` are refused
  because the chain is unknown. `evm:native` and `solana:native` **parse** today as a token whose
  contract is literally the string `native`: `canonical_evm_contract` leaves non-address text as
  found (L242–254), and Solana is identity. Nothing in `resolve_tokens` checks that the contract is
  well formed (`crates/connector-config/src/denomination.rs` L426–481).
- **ADR 0080 decision 7's "`[[tokens]]` still may not" is prose, not code.** No zk-note table
  exists yet ("Nothing is built", ADR 0080 status line). CONFIRMED-DOCS, and confirmed by absence in
  the code.
- **Rate rows need declared tokens on both sides.** `resolve_rate_rows` refuses a `from` or `to`
  absent from `[[tokens]]` with `RateRowUnknownToken` (`denomination.rs` L569–606).
- **Every peering must resolve to a declared token** once any token is declared
  (`ConfigError::PeeringTokenNotDeclared`, `crates/connector-config/src/peering_asset.rs` L20–26,
  `error.rs` L1447).
- **A quote path stays on its own chain and ends at the numeraire.** `resolve_quote` refuses a
  quote on a chain with no settlement table (`TokenQuoteWithoutSettlement`), a leg quoting into
  another chain (`TokenQuoteOffChain`), a pool its chain cannot address (`canonical_pool` covers
  `Evm` hex and `Solana` base58 only) and a path not ending at the node's one numeraire
  (`denomination.rs` L486–560).
- **The poller dispatches on `AssetChain`.** It imports the type
  (`crates/connector-runtime/src/rate_poller.rs` L71). Its cadence is `ttl / 3`, with a 1 s floor
  (L77–101).

**What changes if `[[tokens]]` may name `xrpl:native`.** All **Inference**:

1. **It is required, not optional.** An XRPL peering holds `xrpl:native`. Under
   `PeeringTokenNotDeclared` and `RateRowUnknownToken`, a node that deals XRP must declare it in
   `[[tokens]]`. ADR 0080 avoided this for MINA only because a zk-note table is not a peering. The
   same exemption does not carry over.
2. **As a static row** (`[[tokens]] asset = "xrpl:native"` with no `quote`, plus `[[rates]]` rows):
   - `AssetChain` gains `Xrpl`, and `AssetId` learns the reserved token `native`. That keyword
     should be refused for `evm:` and `solana:`, where it would otherwise be a nonsense contract.
   - `resolve_tokens` accepts the row unchanged.
   - Nothing else moves, because no source is involved.
3. **As a quote**, three more things change:
   - `canonical_pool` needs an XRPL spelling: an AMM account `r…`, or an oracle's
     `(account, document_id)`. `PoolId` in the port is already an opaque string.
   - `settles_on` needs an `[settlement.xrpl]` table.
   - The numeraire rule bites. A path must end at **the** numeraire `AssetId`. An XRPL path ends at
     XRPL RLUSD or USDC, or at "USD" for an oracle, never at `evm:<USDC>`. It works only if the
     node's numeraire is an XRPL stablecoin, or if ADR 0071 gains an equivalence between stablecoins
     on different chains. Today the same-asset USDC-on-Base/USDC-on-Solana hop is handled by having
     **no** rate, not by equivalence (`denomination.rs` L1293–1310).

---

## 3. MINA

[`mina-tokens-and-stablecoins.md`](mina-tokens-and-stablecoins.md) already covers this:

- It found no stablecoin on Mina L1 (§4).
- The first DEX, Solis, went live only in Q1 2026 (L311).
- Its §5.2 point 3 concludes there is "no on-chain TWAP for MINA in this design".

CONFIRMED-DOCS, from that note's primary sources.

[`mina-stablecoin-options.md`](mina-stablecoin-options.md) §1.5 records the only on-chain MINA price
found: zkUSD's whitelisted-oracle median. That project is defunct, and its oracle hosts no longer
answer. CONFIRMED-DOCS, CONFIRMED-LIVE.

**Inference:** with no stable asset on L1, no Mina pool can quote MINA in a numeraire. ADR 0080
decision 7's static row is the only source Mina primary sources support. This note did not look
further.

---

## Unconfirmed, and why

- **DIA's methodology and heartbeat.** The docs page is behind a Cloudflare challenge. The update
  cadence above is inferred from 30 transactions.
- **Band's XLS-47 account on mainnet.** It was not found in Band's docs or the XRPL docs. A full
  ledger scan was not completed.
- **Whether a typical operator's `rippled`/Clio endpoint keeps enough history** for a
  ledger-sampled AMM mean. `online_delete` defaults were not checked.
- **Pool depths** are one read at ledger 107960022 and move continuously.

---

## Implications for the decision

**(A) Static `[[rates]]` rows only, for native coins.**

- For:
  - It matches ADR 0080 decision 7 exactly.
  - It needs no amendment to ADR 0071 (decision 3 already names "no pool at all" as a static-row
    case).
  - It needs no new source, contract suite or chain-per-test harness.
  - It is the only option MINA supports (§3).
- Against:
  - XRP moves daily, so the operator must cover movement between restarts with `spread`.
  - Unlike MINA, an XRPL leg is a **peering** and carries forwarding traffic. It has no open-budget
    cap; the owner declined an extra exposure cap.
- Code cost:
  - `AssetChain::Xrpl`;
  - a reserved `native` token;
  - letting `[[tokens]]` name `xrpl:native` with no quote (§2.1–2.2). It must, because
    `PeeringTokenNotDeclared` requires it.
  - ADR 0080's "`[[tokens]]` may not" therefore cannot be the XRPL rule.

**(B) An XRPL `RateSource` reading XLS-47 oracles.**

- For:
  - It is live on mainnet.
  - It is native, and read over the same `rpc_url` the backend uses. That satisfies decision 3's
    "own settlement chain".
  - Its integer `AssetPrice/10^Scale` is exactly rational.
  - DIA publishes XRP/USD and USDC/USD and RLUSD/USD, so a two-leg path to a USD stablecoin
    composes.
  - It can be tested chain-per-test, with `PriceOracle` set in `[amendments]`.
- Against:
  - **It is an attested oracle, not a market.** ADR 0071 admits only TWAPs over named pools. It
    needs an amendment that adds an oracle source class and states its trust (the publisher's
    key).
  - `get_aggregate_price` has **no time window**: one price per oracle, with `time_threshold`
    relative to the newest oracle. Staleness rides entirely on `ttl`.
  - Only one publisher (DIA) is confirmed. With one oracle, trim and median mean nothing.
  - Updates were up to about 5 h 40 min apart in the sample, so a `ttl` short enough to matter
    would take the pair down between updates.
  - The quote is "USD", not a token. Ending at an `evm:` numeraire needs the cross-chain
    equivalence in §2.3.
  - Contract-suite promise 2 has to be restated for an age bound.

**(C) An XRPL `RateSource` reading AMM spot, with guards.**

- For:
  - It is real market depth: about $4.8 M on XRP/RLUSD.
  - It needs no third party.
  - It is on the leg's own chain.
  - Canonical ordering removes MEV.
  - XRPL has no atomic flash loan.
  - `max_move` and `ttl` still apply.
- Against:
  - **ADR 0071 rejects it by name**: "Spot reads … Rejected: a spot ratio is manipulable inside one
    block … the read is a TWAP; anything else is not a rate" (Considered options). Its `slot0`
    falsifier states the same intent. Moving XRP/USDC 10% costs about $6.5 k.
  - Adopting it would reverse a considered option, not amend a decision.
- **Variant C′, a ledger-sampled mean over historical `amm_info`.** It meets the letter of "TWAP"
  better.
  - It needs a history-keeping endpoint and N reads per refresh.
  - Its resistance is sampling, not a cumulative accumulator.
  - Its tests would need real seconds between ledgers.
  - It still conflicts with decision 6's "a pool without that interface [`observe()`] is not a
    source", if that sentence binds every implementation and not just the first.
- In both forms, an XRPL quote must end at an XRPL stablecoin, which meets the numeraire problem
  in §2.3.

**Inference, overall:** (A) is the only option that fits the records as written. (B) is the
strongest candidate if the owner wants XRP self-sourced, but it is an ADR 0071 amendment that adds a
source class, with a single confirmed publisher. (C) contradicts a considered option of ADR 0071,
and (C′) is a weaker TWAP than the one the record was built around. Any self-sourced option also
needs a decision on the numeraire across chains, which (A) avoids.
