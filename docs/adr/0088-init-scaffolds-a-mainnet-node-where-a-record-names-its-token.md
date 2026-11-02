# `init` scaffolds a mainnet node where a record names its token

**Status:** Accepted (owner decisions, 2026-11-02, #1647). It is written ahead of the rows it permits, and `connector init`'s mainnet rows, Robinhood Chain's rows and the guide's section land under the same spec. It **reverses** the rule in `init`'s module documentation that only `--devnet` exists. It **amends** [0085](0085-whose-code-a-channel-may-bind-to.md): decision 5 reaches the channel contract and not a token issuer, and decision 3's table holds one EVM entry rather than a row per chain (both noted in place there). The evidence is in two research notes: [`robinhood-chain-x402-settlement.md`](../research/robinhood-chain-x402-settlement.md), which wins where they differ, and [`robinhood-chain.md`](../research/robinhood-chain.md).

**Scope:** connector architecture — what the `connector init` command will write, and how 0085's mainnet rule is read. Nothing changes on the wire, in the vectors or in the settlement path. See the [ADR index](README.md).

**Falsifier:** `crates/connector-domain/src/**/*.rs` matching `(?i)robinhood` — this record says Robinhood Chain is scaffolding and not a settlement family, so naming it in the domain would mean a chain-specific variant or rule exists.

**Falsifier:** `crates/connector-config/src/**/*.rs` matching `(?i)robinhood` — this record says a named network is not a gate and adds no config section, so naming the chain in the loader would mean a `[settlement.robinhood]` table or an allowlist exists.

**Falsifier:** `crates/connector-settlement-evm/src/**/*.rs` matching `(?i)robinhood` — this record says the chain runs on the existing EVM backend unchanged, so naming it there would mean a chain-specific path exists and adding the chain was not a config change.

**`connector init` may write a mainnet node on Base, Solana and Robinhood Chain, because the operator must say both `--mainnet` and `--chain` and this record names each token, so nothing is guessed. A mainnet row exists only where a record names its token. Robinhood Chain's dollar is Paxos's USDG, the first default that is not USDC. Decision 5 of 0085 reaches the channel contract and not a token issuer: issuer freezes are an accepted risk, stated in the guide, and the guide is the only place a trust warning needs to live.**

## Context

`init`'s module documentation says only `--devnet` exists, because a mainnet node "settles real money in a token the operator chooses" and nothing should guess that token. The result is that an operator who wants any mainnet, or Robinhood Chain at all, works out the whole table alone. On Robinhood Chain that includes facts nobody would guess: there is no Circle USDC, the dollar is USDG, and USDG's EIP-712 domain is `Global Dollar` / `1`.

The binary already settles there. It holds no chain allowlist and no USDC assumption, and it boots from config on both Robinhood networks (§ Evidence). What was missing is the product surface, and adding that surface reverses a stated rule and raises a question 0085 left open, since both issuers can freeze the tokens these rows name.

## Decision

### 1. `init` scaffolds a mainnet node, and a record names each token

The earlier rule refused to guess. This one guesses nothing either:

- **The operator states both facts.** `--mainnet` without `--chain` is refused, as are `--devnet --mainnet` and an unknown `--chain`.
- **A row's values are read, not chosen.** Each was read live on 2026-11-02, and boot still checks an EVM table's chain id, decimals and token domain against the chain, so a row that goes stale is refused rather than obeyed.
- **The report says what it is.** A mainnet report states that the node settles real money, that the default RPC is not for production, and that the plaintext switch must go before the node is exposed. `--rpc-url` replaces the default.
- **The scaffold stays on loopback.** A mainnet scaffold is a node to start from, not a published one.

**A mainnet row exists only where a record names its token.** A new mainnet row needs a new record or an update to this one. These are the three:

| Row             | Network       | Token                                               | Decimals | EIP-712 domain        |
| --------------- | ------------- | --------------------------------------------------- | -------- | --------------------- |
| Base            | `eip155:8453` | USDC `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913`   | 6        | `USD Coin` / `2`      |
| Robinhood Chain | `eip155:4663` | USDG `0x5fc5360D0400a0Fd4f2af552ADD042D716F1d168`   | 6        | `Global Dollar` / `1` |
| Solana          | mainnet-beta  | USDC `EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v` | 6        | none                  |

**The rule covers scaffolding only.** A named network is not a gate. A hand-written table for any EVM chain with the contract keeps working, and needs no record.

This does not touch [0056](0056-production-is-a-named-empty-tier.md)'s fleet half. The fleet's production tier stays named and empty, and its skeleton stays invalid on purpose. The node `init` writes belongs to whoever ran it.

### 2. Robinhood Chain is two rows of the EVM table, and it settles in USDG

Robinhood Chain is `eip155:46630` and `eip155:4663` under [0086](0086-a-node-settles-on-more-than-one-evm-network.md)'s `[settlement.eip155.<chainId>]`. It adds no settlement family, backend, config section, claim field or vector.

**USDG is the owner's choice, and bridged USDC on the chain is not used.** No Circle USDC exists there. Every other `init` row settles in USDC or, on a devnet, the guide's devnet USDC or mock USDC.

**USDG and USDC are not interchangeable, and the binary will not notice if an operator treats them so.** Under 0086 decision 13, a node that declares no `[[tokens]]` forwards between its tables unconverted. Both tokens have 6 decimals, so a hand-added second table with no `[[tokens]]` boots (§ Evidence) and would forward USDG to USDC at par. `init` stays at one table per scaffold so that it never writes this shape. The guide tells an operator crossing the two to declare `[[tokens]]` and a rate ([0071](0071-a-forward-crosses-a-denomination-at-a-declared-rate.md)).

### 3. 0085 decision 3 holds one EVM entry, and a named network adds no row

0085 decision 3 makes what a voucher binds a table keyed by network, refuses a network with no entry, and its consequences say each new chain adds an entry per network. The EVM half of that was never built that way and is not going to be. x402's address is canonical and the same on every EVM network, so what a voucher binds does not vary by chain id, and 0086 decision 4 checks each table for x402's code on its own.

**So `eip155` holds one entry, x402's canonical address, for every chain id.** The refusal that protects an operator on EVM is that the entry's code is absent or disagrees at boot (`getChannelId` is compared), not that the chain id is unlisted. The binary holds no EVM chain list, and this record does not add one. The table per network still applies where the binding varies: Solana's and XRPL's rows, and any chain deployed at no canonical address.

### 4. Decision 5 of 0085 applied to these rows

All of these are x402's own code (0085 decision 1, kind 1), so decision 4's review and audit do not apply.

| Code                                                         | Authority                                                                                                                 | Under decision 5                                                                                      |
| ------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| `x402BatchSettlement` and its collectors, Base and Robinhood | None. No owner, admin, pause or upgrade function, and it is not a proxy (read on Robinhood's networks; same code on Base) | Passes outright: code nobody can change                                                               |
| `payment-channels`, Solana mainnet-beta                      | solana-foundation's Squads v4 vault `DXtFpbPjcn2hxPnw79x1Pfoj35vXh5AsWBkS37YnXMVv`, 3 of 5, no time lock                  | Passes: already accepted by [0075](0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md) |

The Solana authority is as 0075 read it. The live read on 2026-11-02 confirmed an executable account owned by the upgradeable loader and did not re-read the authority.

Permit2 is present and matches on these chains, but it needs no ruling here. A row's default transfer method is `eip3009`, which USDG was run on (§ Evidence) and Base's USDC is a token of that kind, so a default deposit does not go through Permit2.

### 5. Decision 5 of 0085 reaches the channel contract, not the token issuer

**The owner's reading, 2026-11-02.** Decision 5's question is whose key can change or seize the on-chain code a channel binds to, and its cases are all channel code: Hedera's escrow, a TOON key, `payment-channels`, Mina's zkApp. A token issuer is not one of them. So an issuer's freeze, pause or upgrade power does not fail a mainnet row, and the issuer's key arrangements do not need establishing for a row to ship.

This does more than clarify. Decision 5's words, "move, freeze or replace channel funds or code", would reach a freeze of the escrow address if taken alone, which is why this record amends it rather than merely citing it.

**Why this reading.** The token is not part of what a voucher binds: it is a per-table config value (0086 decision 6), chosen by whoever holds the table. Every regulated dollar has an issuer who can freeze it, so the literal reading would leave no mainnet dollar on any chain, including the USDC that the one third-party mainnet node already settled in under TOON's contracts. An operator choosing a dollar is choosing its issuer, whichever command wrote the table.

**The accepted risk, stated rather than minimised.**

- **USDG.** Paxos is the issuer. The token is a UUPS proxy over implementation `0x68184c449e1a8f34fa18d289737129fd27b66f8f`, so it is upgradeable, and `pause`, `freeze` and `isFrozen` route to facet `0x58ca…E942`. Live on 2026-11-02, `paused()` was false, and neither `x402BatchSettlement` nor the ERC-3009 collector was frozen. **Which keys hold those powers was not established,** and under this decision need not be.
- **USDC.** Circle can likewise freeze or pause its token. This record does not establish how Circle's keys are arranged either.
- **One address holds every EVM channel's escrow for a token.** A freeze of `x402BatchSettlement`, or a pause of the token, stops payouts on every channel in that token at once, whoever the parties are. A freeze here is not one channel's problem.

**What this does not loosen.** Decision 5 still binds the channel contract in full. A single key over channel code fails exactly as before, and a token's issuer gains no standing to be bound to as channel code.

**The same line applies to a chain's operators.** Robinhood's sequencer and its 7-of-8 upgrade path are not a key over `x402BatchSettlement`, which is immutable, but they can stop a voucher being landed. Those are accepted risks of the same kind, and the guide states them too.

### 6. The guide is the only place a trust warning needs to live

The guide states the issuer's freeze and pause and the chain's trust assumptions. Neither the binary nor the report carries them, for these reasons:

- **A warning keyed on a chain id is an allowlist's data.** It would put a list of chains in the binary, and a hand-written table would either escape it or be gated by it. Decision 1 keeps neither.
- **The facts go stale faster than a binary.** A rating or a filter's size changes while a pinned image runs for months. The guide is one edit away from correct, and a stale warning in the binary reads as a current one.
- **The decision is made before the command.** How much collateral to hold on a chain is weighed when choosing it. A line printed once at scaffold time arrives after that choice, and a log line at boot reaches nobody weighing it.
- **The risks are not peculiar to one row.** Issuer freezes apply to USDC on Base and Solana too, so a warning on Robinhood's row alone would imply the others have none.

What a mainnet report does say is about the operator's own act: real money, a non-production default RPC, the plaintext switch. Those are facts about this node, which is why they differ in kind from a judgement about a chain.

**The accepted cost:** an operator who runs `init --mainnet --chain robinhood` without reading the guide sees no statement of the chain's or the issuer's trust assumptions.

## Evidence

Everything below was read or run on 2026-11-02, and no transaction was sent to either Robinhood network. This is the record that adding Robinhood Chain stays a config change.

**The same code as Base.** Runtime compared at each canonical address:

| Contract                  | Size         | Differs only in                                  | Bytes, against Base mainnet | Bytes, against the committed runtime (Base Sepolia) |
| ------------------------- | ------------ | ------------------------------------------------ | --------------------------- | --------------------------------------------------- |
| `x402BatchSettlement`     | 11,175 bytes | Cached chain id and EIP-712 separator immutables | 8005–8006, 8017–8048        | 8004–8006, 8017–8048                                |
| `ERC3009DepositCollector` | 1,150 bytes  | Nothing: byte-identical                          | none                        | none                                                |
| `Permit2DepositCollector` | 2,467 bytes  | Nothing: byte-identical                          | none                        | none                                                |
| Permit2                   | 9,152 bytes  | Cached chain id and separator immutables         | within 6975–7014            | 6974–6976, 6983–7014                                |

The two byte columns differ only by baseline: Base mainnet's chain id shares its high byte with Robinhood's, and Base Sepolia's does not. The separators recompute exactly from each chain id, so those ranges hold nothing else. `eip712Domain()` answers `x402 Batch Settlement` / `1` on both Robinhood networks.

**The binary, unmodified.** No chain id is allowlisted, hardcoded or mapped in any path that gates a network. `connector` reached `connector listening` on the public RPC of 4663 and of 46630, and with 4663 beside Base Sepolia in one config. Boot refused the wrong domain, wrong decimals and a table keyed for the wrong chain, each by name, against the live chain.

**The lifecycle, on a local anvil fork of 4663.** Two unmodified nodes ran open, top-up, paid packets, claim, settle and withdraw in USDG, and the balances conserved. The ERC-3009 route ran on real USDG code: `AuthorizationUsed`, then the transfers through the collector, then `ChannelCreated` and `Deposited`. **That fork is the token's evidence.** The live USDG channels on 4663 do not stand in for it: all 19 ended in `Refunded`, named one receiver and used no EIP-3009, so they do not demonstrate the connector's path.

**The other mainnets.** Base: `eth_chainId` 8453, code at x402's address, USDC answering `USD Coin`, `2` and 6 decimals. Solana mainnet-beta: `CHNLx…` executable under the upgradeable loader, and `EPjF…` a 6-decimal mint owned by the SPL Token program.

**Still unverified, and accepted by the owner:**

- A transaction through Robinhood's live sequencer, on either network.
- A node booted on Base mainnet or Solana mainnet-beta, and a comparison of Solana's program bytes on that day.
- The shipped image on Robinhood Chain.
- `toon-client` paying there. Its offer parser takes any `eip155:` network and its presets name only 84532.
- A facilitator relaying a deposit, so no new EVM row writes one and a payer needs its own ETH to open a channel ([0076](0076-the-operator-names-the-facilitator-and-pays-its-gas.md)).
- USDG's EIP-3009 facet source, whose behaviour was run on a fork and not read.
- Who deployed on mainnet, and the public RPC's real rate limit.

No live rehearsal is required before any row ships (owner decision). The EVM rows get no chain test of their own, because the existing EVM suites already cover this code and backend.

## Considered options

- **Keep `--devnet` only.** Rejected: every mainnet operator then copies values by hand, and on Robinhood Chain that means learning the token, the domain and that `version()` reverts. Refusing to guess is kept, by requiring `--chain`.
- **Let `--mainnet` default to Base.** Rejected: it picks a chain and a token for real money.
- **Read decision 5 literally and refuse issuer-controlled tokens.** Rejected: no mainnet dollar survives it (decision 5).
- **Establish each issuer's key arrangements before shipping a row.** Rejected: it would only move the question, since a good answer today can change by an upgrade tomorrow, and the operator's choice of issuer does not depend on it.
- **Bridged USDC on Robinhood Chain.** Rejected by the owner (2026-11-02).
- **A trust warning in the report or at boot.** Rejected (decision 6).
- **A new settlement family or an allowlist of named networks.** Rejected: the binary needs neither, and an allowlist would refuse hand-written tables that work today.
- **Name a third-party facilitator as a default.** Rejected: whether either known one relays a deposit toward a channel this node admits is unverified, and 0076 makes it the operator's choice.

## Consequences

- **Module documentation, usage text and the guide's `connector init` section change** with the rows, since each states the old rule or its effect.
- **A new default dollar is possible and has a cost.** An operator who copies Base's values onto Robinhood Chain is refused at boot by name. The refusal's advice to run `version()` is reworded, because USDG reverts that call.
- **A peering needs a table on the same token at both ends**, or it is `TokenNotShared`. Whether a USDG leg earns anything depends on counterparties holding USDG on 4663, and nothing here creates any.
- **An issuer freeze of the escrow address locks that token's channels on that chain**, and this repository has no remedy for it.
- **The guide owes its trust section.** L2BEAT was read through a summarising tool, so it is re-read directly before publishing. Robinhood's terms bar masking identity with a VPN or proxy, and the guide points an operator at them before `rpc_via_socks_proxy`.
- **Out of scope:** a `--chain` for XRPL or any other EVM network, several chains in one `init`, a fleet node on Robinhood Chain, and any change to `toon-client`, `rig` or `swap`.

## Sources

- [`robinhood-chain-x402-settlement.md`](../research/robinhood-chain-x402-settlement.md): the deployment, the bytecode, the boot and fork runs. Wins where the notes differ.
- [`robinhood-chain.md`](../research/robinhood-chain.md): the chain, USDG's issuer controls, the trust assumptions and Robinhood's terms.
- #1647: the spec, its owner decisions and its Further Notes, including the comparison with Base mainnet and the live reads of Base and Solana mainnet.
- [0075](0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md) for the `payment-channels` authority as found, [0085](0085-whose-code-a-channel-may-bind-to.md) for the rule, and [0086](0086-a-node-settles-on-more-than-one-evm-network.md) for the table and decision 13.
