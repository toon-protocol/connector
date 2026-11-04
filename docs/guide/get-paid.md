# 3. Get paid

A price makes a route cost something. A **settlement backend** is what lets
anyone actually pay it. There are three families of chain — EVM, Solana and [XRPL](#xrpl) — and a node
may carry any of their tables, accepting vouchers on all of them at once. Every channel
is an **x402 `batch-settlement` channel**
([ADR 0075](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md)):
on EVM that is x402's audited `x402BatchSettlement` contract, and on Solana
solana-foundation's `payment-channels` program. Both are **constants of the
binary**, the same address on every network, so there is nothing chain-specific
to name here beyond your own RPC, token and key.

**[`docs/operators/x402-batch-settlement.md`](../operators/x402-batch-settlement.md)
is the full guide**: what a paying client does from greeting to exit, every
x402 key with its default and refusal, who pays gas and why, choosing a
facilitator, and which tokens work. This step is the short version.

> [!NOTE]
> **Upgrading a node that still holds TOON channels?** Read
> [`docs/operators/draining-toon-channels.md`](../operators/draining-toon-channels.md)
> first — a journal holding a `toon-channel` entry is refused at boot, by name.

```toml
# EVM — Base Sepolia. These are live addresses, not placeholders.
# The table key is the chain id, checked against the RPC at boot.
[settlement.eip155.84532]
rpc_url               = "https://base-sepolia-rpc.publicnode.com"
token_address         = "0x0C996d7c934c79a6255254875607Fe69df25C0E1"  # the token every price on this node is in
decimals              = 6              # units per token: 6 means 1,000,000 = 1.00
asset_eip712_name     = "USDC"         # the token's own EIP-712 domain, from its issuer or the
asset_eip712_version  = "2"            # tables here; boot checks them against DOMAIN_SEPARATOR()
# min_withdraw_delay_secs = 86400      # default one day; 900 at least, 30 days at most
# asset_transfer_method = "eip3009"    # how a deposit moves the token; "permit2" for one without ERC-3009
# facilitator_url = "https://..."      # the x402 facilitator payers relay deposits through; you pay its gas

[settlement.eip155.84532.key]
key_file = "/app/data/settlement.key"

# Solana — public devnet.
[settlement.solana]
rpc_url               = "https://api.devnet.solana.com"
token_address         = "34eSxY7qxQ4GzyhDJ8GpUcTz1WWzruGbJbR8q6TtxfQU"  # an SPL mint; Token-2022 is refused
decimals              = 6
min_sponsored_deposit = 1000000        # base units: the smallest deposit whose open you will pay rent for
# min_grace_period_secs = 86400        # default one day; 900 at least

[settlement.solana.key]
key_file = "/app/data/settlement-solana.key"
```

Copy the token address rather than picking your own: a voucher resolves
against **one** token, so every node that might accept a given voucher has to
name the same one. The two `asset_eip712_*` keys and `min_sponsored_deposit`
have no default and are **required** wherever their table exists. The EIP-712
name and version belong to the token, so don't carry the devnet's to another
network: Base mainnet's native USDC (`0x833589fC…2913`) is
**`"USD Coin"`** / `"2"`, not `"USDC"`. Take them from the token's issuer or the
tables in this guide, because not every token answers a `version()` call. Under
the default `eip3009`, boot refuses a name and version that do not reproduce the
token's `DOMAIN_SEPARATOR()`. Under `permit2` the domain is not checked, so a
wrong one shows up only as every client's deposit failing. `0` is refused for
`min_sponsored_deposit`, because it bounds an endpoint that spends your SOL.

|                  | EVM                                                                                                                                                                                   | Solana                                                                                                                                                                   |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Chain            | Base Sepolia, chain id `84532`                                                                                                                                                        | public devnet (`solana:devnet`)                                                                                                                                          |
| RPC              | `https://base-sepolia-rpc.publicnode.com`                                                                                                                                             | `https://api.devnet.solana.com`                                                                                                                                          |
| Channels live in | [`x402BatchSettlement`](https://sepolia.basescan.org/address/0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003) — the same address on every network                                          | [`payment-channels`](https://explorer.solana.com/address/CHNLxYvVA28MJP9PrFuDXccuoGXAx7jBacfLEkahyGsX?cluster=devnet) — the same program id on every cluster             |
| Token            | [`0x0C996d7c934c79a6255254875607Fe69df25C0E1`](https://sepolia.basescan.org/address/0x0C996d7c934c79a6255254875607Fe69df25C0E1) — devnet USDC (Circle FiatToken v2.2, ERC-3009), 6 dp | [`34eSxY7qxQ4GzyhDJ8GpUcTz1WWzruGbJbR8q6TtxfQU`](https://explorer.solana.com/address/34eSxY7qxQ4GzyhDJ8GpUcTz1WWzruGbJbR8q6TtxfQU?cluster=devnet) — mock USDC mint, 6 dp |
| Funding the key  | Base Sepolia ETH for gas; devnet USDC from the [devnet faucet](https://faucet.devnet.toonprotocol.dev)                                                                                | devnet SOL (`solana airdrop 1 <address> -u devnet`); mock USDC from the same faucet                                                                                      |

There is no Solana _testnet_ deployment: `payment-channels` is on devnet and
on mainnet-beta, nowhere else. **Mainnet exists on both chains** — used by one
third-party operator's node, not by this repository's fleet. The devnet table
above is what the fleet and the faucet serve; a node on mainnet funds itself.
Because the x402 contract and program are the same address everywhere, there
is no deployment record to get wrong here — the one thing that still has to
match the network is the **token**: point `token_address` at Base mainnet's
USDC or Solana mainnet-beta's, never the devnet one
([`connector init`'s table](run-a-node.md#the-fastest-start-connector-init) has
both). TOON's own retired
contracts still have deployment records, now under
[`docs/deployments/`](../deployments/) (moved out of `packages/contracts`
and `packages/solana-program` when #1386 removed those packages), but this
build reads none of them — see
[the drain procedure](../operators/draining-toon-channels.md) if that is
why you are here. The fleet's production tier is still a **named, empty tier**
([ADR 0056](../adr/0056-production-is-a-named-empty-tier.md)): no fleet
machine, no fleet key, and
[`connector.production.toml`](../../deploy/connector-rust/connector.production.toml) is
a skeleton in which every value fails to load on purpose. Do not fill it in.

`decimals` is what turns a `price` into money. Every `price` in the config is a
count of the token's smallest unit, and `decimals` says how many of those make
one whole token — so with `decimals = 6`, `price = 1000` is 0.001 of the token,
and a route meant to cost ten cents of USDC is `price = 100000`. The node reads
the token's own decimals at boot and **refuses to start** if the config
disagrees, because a wrong `decimals` is not a rounding error: it misprices
every route by a factor of ten or more.

Every other settlement value is verified against the chain the same way, which is
why a running node has no `--network` flag: which chain a node is on **is** these
values and nothing else. `connector init`'s `--devnet`, `--mainnet` and `--chain`
only choose what it writes into this file. The node reads the chain id off the RPC and confirms
`x402BatchSettlement`'s code is present at its fixed address before it will
serve (ADR 0075 decision 1). The Solana backend proves the program account
behaves like `payment-channels`, that `token_address` is an SPL mint, and asks
the chain its own genesis hash so a voucher declaring the wrong cluster is
refused.

## Robinhood Chain

Robinhood Chain is an Arbitrum Nitro L2 on Ethereum, and an EVM table like any
other. `connector init --devnet --chain robinhood` and
`--mainnet --chain robinhood` write these values, or you can write them by hand:

|                 | Testnet                                                                                                                                                                                                           | Mainnet                                                                           |
| --------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------- |
| Chain id        | `46630`                                                                                                                                                                                                           | `4663`                                                                            |
| RPC             | `https://rpc.testnet.chain.robinhood.com`                                                                                                                                                                         | `https://rpc.mainnet.chain.robinhood.com`                                         |
| Token           | USDG `0x7E955252E15c84f5768B83c41a71F9eba181802F`                                                                                                                                                                 | USDG `0x5fc5360D0400a0Fd4f2af552ADD042D716F1d168`                                 |
| Decimals        | `6`                                                                                                                                                                                                               | `6`                                                                               |
| EIP-712 domain  | `Global Dollar` / `1`                                                                                                                                                                                             | `Global Dollar` / `1`                                                             |
| Funding the key | ETH for gas: [Arbitrum's launch post](https://blog.arbitrum.io/robinhood-chain-testnet/) names [Robinhood's faucet](https://faucet.testnet.chain.robinhood.com). USDG: [Paxos's faucet](https://faucet.paxos.com) | ETH for gas, and USDG. Both are yours to bring; nothing here funds a mainnet node |

```toml
[settlement.eip155.4663]
rpc_url               = "https://rpc.mainnet.chain.robinhood.com"  # public and rate-limited: use a provider's
token_address         = "0x5fc5360D0400a0Fd4f2af552ADD042D716F1d168"
decimals              = 6
asset_eip712_name     = "Global Dollar"
asset_eip712_version  = "1"

[settlement.eip155.4663.key]
key_file = "/app/data/settlement.key"
```

This page does not state either faucet's limits: Robinhood's refused an automated
read when this was written, so its current terms are unconfirmed, and Arbitrum's
post is from the testnet's launch. Check both before you rely on them. TOON's
devnet faucet does not serve this chain, and no facilitator is written for it on
either network, so a payer needs its own ETH to open a channel.

**USDG is not USDC.** There is no Circle USDC on this chain. Its dollar is Paxos's
USDG, so copy these values rather than Base's: boot refuses Base's domain on this
token, by name. USDG's domain is one you can't look up by asking: its `version()`
call reverts, and the version is known only because `"Global Dollar"` and `"1"`
hash to its `DOMAIN_SEPARATOR()`. Prices count USDG's base units, so `price = 1000`
is 0.001 USDG.

**A peering needs the same token at both ends.** Your counterparty needs a table
on this token on this network, or opening the channel fails with `TokenNotShared`.

**USDG and USDC are not interchangeable, and the binary will not notice if you
treat them so.** Reaching USDC elsewhere is a
[dealing boundary](../adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md)
with a rate you declare. That rate is demanded only once you declare tokens: a
node with **no `[[tokens]]` forwards between its tables unconverted**
([ADR 0086](../adr/0086-a-node-settles-on-more-than-one-evm-network.md) decision
13). Both tokens have 6 decimals, so a USDG table added beside a USDC one boots
and forwards at par, whatever a dollar on either side is worth to you. Declare
`[[tokens]]` for both and a rate for the pair, as the
[XRPL example below](#xrpl) does; `local/dealing` is a worked one.

**What you are trusting**, as [L2BEAT](https://l2beat.com/scaling/projects/robinhood)
and [Robinhood's terms](https://docs.robinhood.com/chain/terms-of-service) stated it
on 2026-11-02. Weigh it when you decide how much to hold here, and re-read both,
since these facts change faster than this page:

- **One sequencer, run by Robinhood.** A voucher is worth what you can land, and
  landing it goes through that sequencer.
- **Below Stage 0.** L2BEAT rates the chain "not even a Stage 0 project". Only two
  whitelisted actors can submit fraud proofs.
- **Instant upgrades.** A 7-of-8 multisig can upgrade core contracts with no delay,
  so L2BEAT gives users no exit window. It reaches the chain beneath
  `x402BatchSettlement`, not that contract, which is immutable.
- **A transaction filter that defeats forced inclusion.** An authorised filterer
  can register a transaction hash, and that transaction then fails, even when it was
  force-included from L1, so L2BEAT calls forced inclusion "not a reliable
  censorship escape hatch".
- **Address blocking.** Robinhood reserves the right to restrict or block specific
  wallet addresses from the chain.
- **The issuer.** Paxos can freeze or pause USDG, and the token is upgradeable.
  Circle can do the same to USDC on Base and Solana, so this risk isn't peculiar to
  this chain. Every channel in a token escrows at one address, so a freeze of
  `x402BatchSettlement`, or a pause of the token, stops payouts on all of them at
  once, and this repository has no remedy for it.

> [!NOTE]
> **Before you set `rpc_via_socks_proxy` on this table,** read Robinhood's
> [terms](https://docs.robinhood.com/chain/terms-of-service). They bar using a VPN,
> proxy or other tool to mask your identity, location or IP address.

**How far this has been run.** x402's contracts here match Base's except for their
chain id and domain separator, and an unmodified node has booted on both public
RPCs and run a whole channel's life in USDG on a local fork of mainnet
([ADR 0088](../adr/0088-init-scaffolds-a-mainnet-node-where-a-record-names-its-token.md#evidence)).
Nobody has yet sent a transaction through the live sequencer from this binary, run
the shipped image on this chain, or paid from `toon-client`, whose presets name only
Base Sepolia.

## XRPL

XRPL is a third family, and the odd one out in three ways: there is **nothing
to deploy** (a channel is the ledger's own `PayChannel`), a channel holds **XRP
and nothing else**, and a claim **binds no network**
([ADR 0087](../adr/0087-xrpl-settles-over-the-ledgers-payment-channels.md)).
Each network is its own table, keyed by the `network_id` the ledger reports:
`0` mainnet, `1` testnet, `2` devnet.

```toml
[settlement.xrpl.1]                      # testnet
rpc_url = "https://s.altnet.rippletest.net:51234"
# min_settle_delay_secs  = 86400         # default one day; must exceed 901
# redemption_safety_secs = 60            # default 60; below the delay
# max_fee_drops          = 100000        # the most one transaction may pay

[settlement.xrpl.1.key]
key_file = "/app/data/settlement-xrpl.key"
```

There is no `token_address`, `decimals` or contract to write. The key file holds
an XRPL **family seed** (`s…` for secp256k1, `sEd…` for Ed25519), not 32 bytes:
`openssl rand -hex 32` makes a key the node cannot read here. Generate one with
any XRPL tool (`rippled`'s `wallet_propose`, or a wallet's "new account"), keep
it out of git, and mount it like the others. Starting the node does not create
the account — see funding, next.

**Use a different XRPL account on testnet than on mainnet.** An XRPL claim
carries no network id, so a voucher signed on testnet verifies on mainnet too
for anyone who holds a channel there with the same id. The node refuses to boot
if two of its own `[settlement.xrpl.<id>]` tables resolve to one account, but
it cannot see another node, nor a key you reused by hand elsewhere. Give every
network its own seed and never carry a mainnet seed to a testnet box.

**Fund the account before you start.** An XRPL account exists only once
something pays it, and it is useful only above the ledger's reserves:

- the **base reserve** (read off the ledger, not written in the config) keeps
  the account alive — a node whose account is missing, or holds less than it, is
  refused at boot, by name;
- an **owner reserve** (0.2 XRP on 2026-10-28) is locked for every channel this
  node opens, until that channel is deleted — one per peering, for you;
- **fees are paid in XRP**, by the party that sends the transaction. Landing a
  voucher costs you a fee; nobody sponsors it, and no second coin is needed.

On testnet and devnet the ledger's own faucet funds an account. On mainnet you
send the XRP.

The node checks the `network_id` its `rpc_url` reports against the table key at
boot and refuses a mismatch, so a testnet endpoint under `[settlement.xrpl.0]`
cannot start. It does **not** check the PayChan amendment: testnet reports it
disabled while channels work.

**Every XRPL node declares its tokens.** A node with a `[settlement.xrpl.<id>]` table and no
`[[tokens]]` refuses to start, even on one network holding only XRP. Add a row per settlement table;
for an XRP-only node, that is one:

```toml
[[tokens]]
asset = "xrpl:1/slip44:144"
```

**Settling XRP in a peering needs a rate.** A channel holds XRP, so a leg on
XRPL is a dealing boundary ([ADR 0071](../adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md); `local/dealing` is a worked one)
whenever the other side holds anything else. Declare the asset in `[[tokens]]`
under its CAIP-19 name, `xrpl:<id>/slip44:144`, and give the pair a static
`[[rates]]` row; XRPL has no rate source, and with no rate there is no forward:

```toml
[[tokens]]
asset     = "eip155:84532/erc20:0x0C996d7c934c79a6255254875607Fe69df25C0E1"
numeraire = true

[[tokens]]
asset = "xrpl:1/slip44:144"

# 1 drop is 1e-6 XRP; the figure is a mid over base units, as every row is.
[[rates]]
from = "eip155:84532/erc20:0x0C996d7c934c79a6255254875607Fe69df25C0E1"
to   = "xrpl:1/slip44:144"
rate = { numerator = 2, denominator = 1 }

# Required as a set once any [[rates]] row exists: the spread is what dealing
# earns you, and it is also your cover against XRP moving after you declared.
[rate_guards]
spread    = { numerator = 1, denominator = 100 }
ttl_secs  = 300
max_move  = { numerator = 5, denominator = 100 }
```

`xrpl:native`, and every other `<chain>:native` spelling, is refused by name.
XRP on testnet is a different asset from XRP on mainnet, so a forward between two
XRPL tables needs a row too.

**What a payer's exit does.** When a payer sets an `Expiration` to leave, your
node lands its latest voucher _and closes the channel_ in one transaction, so
you are paid and the payer refunded before its delay can run out on you. A payer
that began an exit and then wanted to renew loses that chance. A channel that
carries a `CancelAfter`, an `Expiration` already set, a `SettleDelay` below your
minimum, or a `Destination` that is not your account is refused on its first
voucher.

**You do not list the channels your payers will use, and you could not** — a
client's channel does not exist until that client opens it toward you, long
after your node booted. The settlement section gives this node its on-chain
identity, and it is where a voucher naming a channel you have never heard of
is **resolved from chain** and admitted, which is what makes paying you
permissionless rather than an arrangement. Your node only admits a channel
whose terms are the ones it published:

- **EVM.** The client picks its own `payer` and `salt`. `receiver` and
  `receiverAuthorizer` must both be your settlement address, `token` must be your
  token, and `withdrawDelay` must be at least your minimum. `payerAuthorizer`
  must be **nonzero**. That last rule is stricter than x402, which allows a zero
  one, and the greeting has no field to say so. A channel with a zero one is
  refused on its first voucher. The client's deposit is relayed by an x402
  facilitator, which pays its gas. **You name that facilitator**
  (`facilitator_url`, published as `extra.facilitator`) and pay for its gas as a
  cost of the sale; your node never calls it. A payer holding ETH can deposit
  directly instead. An ERC-3009 token such as USDC deposits with one signature.
  Any other ERC-20 needs `asset_transfer_method = "permit2"` (the node refuses
  to start on `"eip3009"` for a token without ERC-3009) and a one-time
  Permit2 approval, which is gasless only if your facilitator sponsors it. The
  first voucher on a channel your node has not seen carries the full
  `channelConfig`, and the node recomputes the channel id from it.
- **Solana.** The client builds an `open` in which your settlement key is the fee
  payer, `rent_payer` and `payee`. It signs it and posts it to
  `POST /ilp/batch-settlement/solana/open`, which the greeting names as
  `extra.sponsorEndpoint`. The node checks every field, co-signs, submits, waits
  for the channel to confirm, and admits it. This endpoint is **public** and
  unauthenticated, because the buyer is a stranger with no channel yet. Its
  limits are what it will sign, a cap of 8 sponsorships in flight and 1 per payer,
  and an hourly budget of failed opens. Only SPL Token mints are accepted:
  Token-2022 is refused.

**How a voucher is checked.** A voucher has no nonce. It is accepted only if
its amount is **strictly greater** than the last one accepted on that channel,
by at least the route's charge. A replayed voucher buys nothing. A packet to a
free route carries no voucher at all. A Solana voucher with a nonzero
`expiresAt` is refused. A client that lost track of its last voucher asks
`POST /ilp/claim-state` with `"scheme": "batch-settlement"` to learn the
amount it has to beat.

**How you get paid.** Automatically, with no operator write, for what lands on
chain. On EVM, the node claims every held voucher in one `claim` and then
`settle`s to your address every ten minutes. It also watches for
`WithdrawInitiated`, and the moment a payer starts a withdrawal it claims that
channel's latest voucher. That matters because only the amount already
claimed on chain survives the withdrawal. On Solana, the node `settle`s open
channels every ten minutes, or on the next watch pass after that. It
rediscovers its sponsored channels from the chain ninety times inside the
node's minimum grace period, at least every ten seconds and at most every
fifteen minutes: fifteen at the one-day default. The EVM withdrawal watch
runs at the same rate off `min_withdraw_delay_secs`. On a closing channel it lands the latest voucher with
`settle_and_seal` inside the grace period, then `distribute`s, then
`reclaim`s the rent. The one-day minimum delay is how long a delayed or
censored transaction still has to land. `POST /channels/:id/land` is the
manual lever for planned maintenance — see
[the operator surface](operator-surface.md).

> [!WARNING]
> **Fund the settlement key _before_ you start the node**, and know that
> **booting a config is not a dry run**. A Solana backend submits a real
> transaction at `connect`; with no gas the connector exits 1 on a chain error
> that reads like a config bug. With a funded key, starting a node "just to see
> whether the TOML parses" spends real money.

You do not need to build the payer —
[`toon-client`](https://github.com/toon-protocol/toon-client) is that — but two
things help when debugging "why is nobody paying me":

- **An ILP outcome is never an HTTP one.** A `FULFILL` and a `REJECT` both come
  back at HTTP **200**.
- **A caller with no voucher on a priced route gets `402`**, carrying the
  route's **greeting**: a document quoting the same price a real request
  would be charged. Its `accepts[]` holds one `batch-settlement` entry for
  each chain you configured, and nothing else — a `toon-channel` claim is
  refused by name, so a node with no `[settlement]` table can be paid by
  nobody. The price and your node's addresses ride beside it in
  `extensions.toon`. Each entry is valid x402: a CAIP-2 `network`, the
  `asset`, `payTo` set to your settlement address, and an `extra` holding
  everything a stock x402 client needs to open a channel you will accept.
  `GET /ilp` publishes the same facts under `batchSettlements`.

Turning the vouchers you collect into money on chain is
[the operator surface](operator-surface.md)'s job. Before you rely on this,
know what it costs and what you are trusting:

- **One way per channel.** Value moves one way on a channel. To pay a client
  back, open a channel of your own toward the terms it publishes
  (`POST /channels`); its payouts arrive as vouchers on that channel.
- **Solana costs SOL.** Each sponsored channel locks about 0.0047 SOL of rent
  (4,711,920 lamports) until it is reclaimed. Your node's token account for the
  mint must already exist, because the sponsor refuses to open a channel into an
  account that would forfeit its payout.
- **Third-party code.** The EVM contract is ownerless and immutable. That also
  means nothing can rescue an escrow that USDC's blacklist has frozen.
  `payment-channels` can be upgraded — on mainnet-beta by a 3-of-5 Squads
  multisig with no time lock, on devnet by a single keypair. What you risk is
  whatever you have accepted in vouchers and not yet landed on chain.

[`client-edge-spec.md`](../protocol/client-edge-spec.md) §1.3, §1.4, §1.10
and §1.11 are the wire rules, and
[`configuration-spec.md`](../protocol/configuration-spec.md) is the keys. The
voucher's exact bytes are `claim_voucher` in
[`vectors/wire-vectors.json`](../../vectors/wire-vectors.json).

## A zk note (Mina)

A node can also be paid from a **zk note** ([ADR 0080](../adr/0080-a-client-may-pay-from-a-zk-note.md)):
a deposit in a Mina vault that its owner spends through a signed **allowance** rather than a
channel. That makes the node a **zk edge**. It is opt-in, and a node without the table is exactly what
it was. Nothing in this repository proves or sends a Mina transaction: the node only _reads_ the
vault, and verifies proofs payers made elsewhere.

```toml
[zk_note]
vault                   = "B62q…"                       # the vault's zkApp address
network                 = "mina:devnet"                 # or "mina:mainnet"
graphql_url             = "https://…/graphql"           # a Mina daemon: the vault's account
archive_url             = "https://…/graphql"           # an archive node: the vault's actions
# rpc_via_socks_proxy   = true                          # both reads ride socks_proxy, https only
state_key_file          = "/app/data/zk-state.key"      # a Mina secret key, EK…
verification_key_hash   = "17783576495452972973648719307576910001175195573379209737447262132918659765208"
tiers                   = [100000000]                   # nanomina; a subset of zk_toon's tiers
allowance_lifetime_secs = 3600
open_budget_cap         = 1000000000
```

- **Two endpoints, because the vault keeps only the root of its nullifier set.** The daemon
  answers the vault's account (its recent roots, its creation slot, its latest action state); whether a note's
  state is already spent is answered from the vault's actions, which only an archive node lists.
  Both ride the same circuit under `rpc_via_socks_proxy`, and an `http` URL is refused on it unless its
  host is an onion one.
- **The request key is part of the binary.** `verification_key_hash` must be the hash of the key this
  build carries (the one above is the build's today); any other value stops the node at boot, naming the
  key. The key is not on chain, and a build serves one Mina era.
- **Boot reads, and then serves.** The node builds its verifier (a second or two of CPU), reads the
  slot it counts time from and, if `subsidy_budget` is set, the vault's creation slot. Each gets a few attempts;
  a node that cannot make them does not boot. It needs `state_dir`.
- **A failed read refuses one allowance.** A slow, down or wrong endpoint answers that request
  `vault_unavailable`, and the next is served. An archive that is behind the daemon, or that lists
  actions that do not hash to the vault's own action state, is a failed read, never "unused".
- **Until clearances exist, payers can leave only by escape.** A payer's quick exit, a mutual close,
  needs the zk edge's signature letting one state leave to one destination (a **clearance**), and
  this build does not issue them ([#1556](https://github.com/toon-protocol/connector/issues/1556)).
  Until it does, a payer takes their balance out by **escape**: they start it on chain, the vault holds
  it for zk_toon's two-day challenge window so a later state can defeat it, and it pays after. A node
  that offers allowances today is offering a note that costs its owner that wait to leave.

The terms are in [`configuration-spec.md`](../protocol/configuration-spec.md) and the wire in
[`client-edge-spec.md`](../protocol/client-edge-spec.md) §1.12.
