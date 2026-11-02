# Robinhood Chain: what it is, and what settling on it would take

> **Research note, not a decision record.** It decides nothing. The owner asked about "robinhood
> chain" with no specific question, so this note says what the chain is and then what a connector
> would need to settle there. Every source and live read was taken on **2026-11-02**.

**Evidence tags** (as in [`x402-pending-chain-bindings.md`](x402-pending-chain-bindings.md)).
CONFIRMED-CODE means read in verified source, or read live from a chain's RPC or a first-party API.
CONFIRMED-DOCS means read in the owner's own documents. UNVERIFIED means neither, and says why.
**Inference** marks this note's own reasoning, not a sourced fact.

**Pinned.** This repository at `08dbc279`; x402 at
[`x402-foundation/x402@0c58a71c`](https://github.com/x402-foundation/x402/tree/0c58a71cc2b050f8512e3226bfd0ff18a1fbe750).
The commands and results for every live read are in [§4](#4-x402-fit) and the
[appendix](#appendix-live-reads). Robinhood's docs were read through a fetch tool that summarises
pages, so wording there is close to the source but not guaranteed verbatim. Addresses and numbers
that matter were re-read on chain where possible.

---

## Summary

- **Robinhood Chain is an Arbitrum Nitro L2 on Ethereum.** It posts data to Ethereum as blobs, uses
  ETH for gas and has one sequencer, run by Robinhood. Public testnet (46630) launched 2026-02-10 and
  public mainnet (4663) launched 2026-07-01.
- **x402's batch-settlement contracts are already deployed there, on mainnet and testnet.**
  `x402BatchSettlement` is byte-identical to the connector's committed runtime except for the
  chain-specific immutables (chain id and EIP-712 domain separator). On mainnet, 18 USDG channels went
  through their full lifecycle in the last 200,000 blocks (about 5.6 hours).
- **There is no Circle USDC.** Circle does not list the chain, and Robinhood's token page names only
  WETH and USDG. The chain's dollar is **USDG** (Paxos, 6 decimals), which implements EIP-3009,
  including the exact `receiveWithAuthorization` overload x402's collector calls.

**Could the connector settle there today?** Probably yes, with a config table and no code change, but
**nobody has shown it**. This is an inference from reading the boot checks and running their reads
live. No connector was built or booted against this chain, and no packet was sent.

Nothing found would stop a node booting on `[settlement.eip155.4663]`. What stands in the way of real
use:

1. **The asset is not USDC.** A USDG channel needs peers and clients that also hold USDG on 4663, or
   a dealing boundary with a declared rate to reach USDC elsewhere.
2. **Unproven against Nitro.** The `ethers` client's handling of live Nitro blocks and receipts,
   and fee behaviour, are untested. Tier-3 anvil would not catch these (§5).
3. **Trust.** L2BEAT scores the chain below Stage 0, with instant upgrades and a transaction filter
   that also defeats forced inclusion (§6). Whether to accept that is the owner's call, not a code
   question.

---

## 1. What it is

| Property          | Finding                                                                                                                                                                                                                                                              | Tag                                                                   |
| ----------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------- |
| Stack             | "Built on Arbitrum Dedicated Blockchains", i.e. Orbit in L2BEAT's terms; runs Nitro and ArbOS 61 ([docs](https://docs.robinhood.com/chain/), [full node](https://docs.robinhood.com/chain/run-a-full-node), [L2BEAT](https://l2beat.com/scaling/projects/robinhood)) | CONFIRMED-DOCS. Live: `web3_clientVersion` = `nitro/v3.12.2+5f02f3f…` |
| Settlement layer  | Ethereum ([connecting](https://docs.robinhood.com/chain/connecting))                                                                                                                                                                                                 | CONFIRMED-DOCS                                                        |
| Data availability | Ethereum blobs ([connecting](https://docs.robinhood.com/chain/connecting), [L2BEAT](https://l2beat.com/scaling/projects/robinhood))                                                                                                                                  | CONFIRMED-DOCS                                                        |
| Sequencer         | One, operated by Robinhood ([ToS §2.1](https://docs.robinhood.com/chain/terms-of-service)); first-come, first-served, and a higher fee does not buy earlier ordering ([differences](https://docs.robinhood.com/chain/differences-from-ethereum))                     | CONFIRMED-DOCS                                                        |
| Proofs            | BoLD with a permissioned validator set: two validators, run by Offchain Labs and Alchemy ([governance](https://docs.robinhood.com/chain/governance))                                                                                                                 | CONFIRMED-DOCS                                                        |
| Gas token         | ETH ([connecting](https://docs.robinhood.com/chain/connecting))                                                                                                                                                                                                      | CONFIRMED-DOCS                                                        |
| Fees              | L2 execution plus L1 data, both bundled into gas; `eth_estimateGas` covers both ([gas](https://docs.robinhood.com/chain/gas-and-fees)). Live base fee was about 0.02 gwei; `eth_feeHistory` tip samples were all `0x0`                                               | CONFIRMED-DOCS, CONFIRMED-CODE                                        |
| Block time        | Not stated by the docs. Measured: 10,000 blocks in 1,010 s on mainnet (about 0.1 s) and 1,402 s on testnet. Arbitrum's [factsheet](https://forum.arbitrum.foundation/t/arbitrumdao-factsheet-robinhood-chain-mainnet-launch/31041) says "100ms latency"              | CONFIRMED-CODE                                                        |
| Block gas limit   | Live `gasLimit` = `0x4000000000000`                                                                                                                                                                                                                                  | CONFIRMED-CODE                                                        |
| ArbitrumDAO link  | Pays 10% of protocol net revenue under the Arbitrum Expansion Program ([factsheet](https://forum.arbitrum.foundation/t/arbitrumdao-factsheet-robinhood-chain-mainnet-launch/31041))                                                                                  | CONFIRMED-DOCS                                                        |

**EVM deviations** ([differences from Ethereum](https://docs.robinhood.com/chain/differences-from-ethereum)):

- `block.number` is an estimate of the **L1** block; `ArbSys.arbBlockNumber()` is the L2 one.
- `block.prevrandao` and `block.difficulty` are constants. `block.coinbase` is the network fee account.
- `blockhash(n)` is reliable only for recent blocks.
- L1 senders are aliased on L1-to-L2 messages.
- Maximum code size is 96 KB, and init code 192 KB.
- Arbitrum precompiles are present ([addresses](https://docs.robinhood.com/chain/protocol-contracts)).
- **The sequencer excludes transactions tied to sanctioned addresses.** Reads are unaffected.

ERC-4337 is supported ([overview](https://docs.robinhood.com/chain/)). Opcode-level differences beyond
this list were not separately checked.

**Finality** ([transaction finality](https://docs.robinhood.com/chain/transaction-finality)):

| Stage                | When                   | What can still undo it                               |
| -------------------- | ---------------------- | ---------------------------------------------------- |
| Soft confirmation    | Sub-second             | The sequencer posting a batch with a different order |
| Posted to Ethereum   | Minutes                | An Ethereum reorg                                    |
| Ethereum finality    | About 13 min later     | Nothing, by the docs' account                        |
| Canonical withdrawal | 7-day challenge period | Separate from transaction finality                   |

Live, `latest` was 5,132 blocks ahead of `safe` and 8,867 ahead of `finalized`, or about 9 and 15
minutes at the measured rate.

**Inference for the connector.** `send.rs` treats a receipt as confirmation, so on this chain it acts
on a soft confirmation that only the sequencer could reorder. This is not new behaviour, but this note
did not check how the other L2 tables compare, or whether a voucher is ever issued against a deposit
that later moves.

---

## 2. Status

**Launched**

| Network | Chain id | Launched                                                                                                                                                                                                                                                                                       | Public RPC                                | Explorer                                                                             |
| ------- | -------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------- | ------------------------------------------------------------------------------------ |
| Testnet | 46630    | 2026-02-10 ([Robinhood](https://www.robinhood.com/us/en/newsroom/robinhood-chain-launches-public-testnet/))                                                                                                                                                                                    | `https://rpc.testnet.chain.robinhood.com` | [explorer.testnet.chain.robinhood.com](https://explorer.testnet.chain.robinhood.com) |
| Mainnet | 4663     | 2026-07-01 ([Arbitrum factsheet](https://forum.arbitrum.foundation/t/arbitrumdao-factsheet-robinhood-chain-mainnet-launch/31041); Robinhood's [newsroom listing](https://robinhood.com/newsroom/category/crypto/) dates its mainnet post Jul 1, 2026, but the post itself could not be opened) | `https://rpc.mainnet.chain.robinhood.com` | [robinhoodchain.blockscout.com](https://robinhoodchain.blockscout.com)               |

Chain ids and URLs are from [add network](https://docs.robinhood.com/chain/add-network-to-wallet) and
were confirmed live: `eth_chainId` returned `0x1237` and `0xb626`.

- **Providers.** The public endpoints are rate-limited and "not recommended for production". The
  docs recommend Alchemy and list other providers ([connecting](https://docs.robinhood.com/chain/connecting)).
- **Public RPC is not an archive.** `eth_getCode` at block `0x6300000`, about 13 hours back at the
  measured rate, returned `historical state … is not available`.
- **Faucet.** Arbitrum's blog names `faucet.testnet.chain.robinhood.com`, dispensing testnet ETH and
  test stock tokens ([blog](https://blog.arbitrum.io/robinhood-chain-testnet/)). Robinhood's docs pages
  read here list no faucet. The host answered HTTP 429 to a plain request, so it is up but its current
  terms are unverified.
- **Service status.** [status.robinhoodchain.offchain.io](http://status.robinhoodchain.offchain.io/) is
  linked from the docs. It was not read.

**Announced or not found**

- Robinhood's testnet post planned "a mainnet launch later in 2026", since delivered.
- No published plan for USDC on the chain was found (§3).
- As far as was checked, no native chain token has been announced. The only source seen is secondary,
  from search results, and dates to mid-July 2026. **Unverified.**

---

## 3. Assets

### USDC

**No Circle-native USDC.** Circle's
[USDC contract addresses](https://developers.circle.com/stablecoins/usdc-contract-addresses.md) has no
Robinhood row on mainnet or testnet, and does not mention Robinhood (CONFIRMED-DOCS). Robinhood's
[token contracts](https://docs.robinhood.com/chain/contracts) page lists WETH and USDG, and its
[bridging](https://docs.robinhood.com/chain/bridging) page does not mention USDC. The address Circle
uses on Arbitrum One, `0xaf88…5831`, has no code on 4663 (live: `0x`).

**Bridged USDC: not established.** The docs name no canonical bridged USDC. Testnet's explorer shows
many tokens calling themselves USDC, all unofficial as far as this note can tell. The mainnet
explorer's API sits behind a Cloudflare challenge, so a token search there could not be run.
**Unverified**, including whether third-party bridged USDC circulates on mainnet.

### USDG, the chain's dollar

| Network | Token                                                                                         | Decimals | Source                                                                                                                  |
| ------- | --------------------------------------------------------------------------------------------- | -------- | ----------------------------------------------------------------------------------------------------------------------- |
| Mainnet | `0x5fc5360D0400a0Fd4f2af552ADD042D716F1d168`, supply control `0xdf5F…25D4`, OFT `0x0d54…28d1` | 6 (live) | [Paxos](https://docs.paxos.com/guides/stablecoin/usdg/mainnet), [Robinhood](https://docs.robinhood.com/chain/contracts) |
| Testnet | `0x7E955252E15c84f5768B83c41a71F9eba181802F`, supply control `0x4549…f54C`                    | 6 (live) | [Paxos](https://docs.paxos.com/guides/stablecoin/usdg/testnet)                                                          |

Live mainnet supply is about 660.4 million USDG. It is bridged via LayerZero OFT/Stargate
([bridging](https://docs.robinhood.com/chain/bridging)).

**EIP-3009 and permit** (CONFIRMED-CODE, by live reads on chain):

- **Routing.** The token is a UUPS proxy with a verified
  [`USDG` implementation](https://sourcify.dev/server/v2/contract/4663/0x68184c449e1a8f34fa18d289737129fd27b66f8f?fields=abi,compilation)
  that dispatches through **facets**, so its implementation ABI lists neither EIP-3009 nor permit.
  `getFacet(bytes4)` routes `receiveWithAuthorization` (both overloads), `transferWithAuthorization`
  (both), `authorizationState`, `cancelAuthorization`, `permit` and `nonces` to facet `0x780d…1309`,
  which is not verified on Sourcify. Testnet routes the same selectors to its own facet.
- **The collector's overload.** x402's committed `ERC3009DepositCollector` contains the selector
  `0x88b7ab63`, the `bytes` overload of `receiveWithAuthorization`. Called with a dummy signature, USDG
  reverts `0x8baa579f` (`InvalidSignature()`), where an unknown selector reverts `0x800ab12c`. So the
  call reaches signature checking.
- **EIP-712 domain.** `{name: "Global Dollar", version: "1", chainId: 4663, verifyingContract: token}`
  hashes to `0x7a3d7400…2036`, which equals the on-chain `DOMAIN_SEPARATOR()`. `version()` reverts, so
  the version is only known from that hash.
  The connector needs no `version()` getter: `asset_eip712_domain_matches` compares `DOMAIN_SEPARATOR()`
  with the configured name and version (`batch_payer.rs`).
- **Not read:** the EIP-3009 facet's source. Its behaviour is inferred from its selectors, the revert
  above, and the 18 channels in §4. Open x402 PRs
  [#3761](https://github.com/x402-foundation/x402/pull/3761) and
  [#3770](https://github.com/x402-foundation/x402/pull/3770) report the same domain, and
  [#3761](https://github.com/x402-foundation/x402/pull/3761) reports EIP-3009 used on mainnet.

**Issuer controls.** `pause`, `freeze` and `isFrozen` route to facet `0x58ca…E942`.
[Verified source](https://sourcify.dev/server/v2/contract/4663/0x68184c449e1a8f34fa18d289737129fd27b66f8f?fields=sources)
reverts `transferFrom` for a frozen `msg.sender`. The token is upgradeable. Paxos's
[terms](https://www.paxos.com/terms-and-conditions/stablecoin-terms-conditions) reserve freezing, as
reported by a search summary; that page was not opened directly. Live: `paused()` is false, and neither
`x402BatchSettlement` nor the ERC-3009 collector is frozen.

**Inference.** Escrow for every USDG channel sits at the one `x402BatchSettlement` address, so a freeze
of that address, or a pause of the token, would stop payouts on every channel at once. USDC has the same
shape on other chains, so this is not unique to USDG.

### Tokenized stocks

- **What they are.** Debt securities issued by Robinhood Assets (Jersey) Limited, giving economic
  exposure only, with no rights in the underlying ([stock tokens](https://docs.robinhood.com/chain/stock-tokens)).
- **Count.** Robinhood's [assets API](https://api.robinhood.com/rhj/assets) returned 203 assets, all
  active, all on chain 4663, all 18 decimals.
- **Standard.** ERC-20 plus ERC-8056's `uiMultiplier()`: splits and dividends change a multiplier, not
  raw balances, and the tokens do not rebase ([building](https://docs.robinhood.com/chain/building-with-stock-tokens)).
- **Primary issuance.** Authorised Participants only, after KYB.
- **Geography.** Not registered under US securities law and may not be offered, sold or delivered in
  the United States. The docs also note restrictions in Canada, the UK and Switzerland
  ([stock tokens](https://docs.robinhood.com/chain/stock-tokens)).

**The docs do not describe the on-chain gates; the code does.** AAPL
([`0xaF3D…93f9`](https://docs.robinhood.com/chain/stock-token-apis)) is a beacon proxy to a verified
[`Stock`](https://sourcify.dev/server/v2/contract/4663/0xb35490d6f9163DE4F80d88dc75c3516eb64C5aE2?fields=abi,compilation)
implementation (other tokens' beacons were not checked). In it (CONFIRMED-CODE):

- `transfer` and `transferFrom` carry `onlyNotPaused` and `onlyNotBlocked` on sender, recipient and
  spender, where the pause includes a global `IAccessControlsRegistry` pause. So there is a **blocklist
  and a pause, not an allowlist**.
- `adminBurn(from, amount)` lets a role burn anyone's tokens without those checks.
- `permit` and `DOMAIN_SEPARATOR` exist. **Neither EIP-3009 selector is in the bytecode.**

**Inference.** Stock tokens could only enter an x402 channel over `asset_transfer_method = "permit2"`.
Doing so would put a securities-law-restricted, blockable, admin-burnable token behind a payment
channel, where the x402 contract becomes the holder. Nothing in this repository needs that, and it
would raise legal questions this note cannot answer. **This note treats them as out of scope for
settlement**, and that is a judgement rather than a finding.

---

## 4. x402 fit

**Yes: `x402BatchSettlement` is on both networks, at the canonical address, running the program the
connector binds to.**

**What was run** (read-only, 2026-11-02), for `R` in `https://rpc.mainnet.chain.robinhood.com` and
`https://rpc.testnet.chain.robinhood.com`:

```bash
curl -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"eth_getCode","params":["0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003","latest"]}' $R
```

Each result was then diffed against `crates/connector-settlement-evm/contracts/x402/*.runtime.hex`,
which `PROVENANCE.md` records as read from Base Sepolia.

| Contract                                | Mainnet and testnet result                                                                                                                                                                                  |
| --------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `x402BatchSettlement` `0x4020…0003`     | Code present, 11,175 bytes, same length. Differs only in the chain id and domain separator immutables                                                                                                       |
| `ERC3009DepositCollector` `0x4020…0004` | Code present, **byte-identical** to the committed file on both networks                                                                                                                                     |
| `Permit2DepositCollector` `0x4020…0005` | Code present, **byte-identical** to the committed file on both networks                                                                                                                                     |
| Permit2 `0x0000…8BA3`                   | Code present. Differs only in its chain id and domain separator immutables. Also [listed by Robinhood](https://docs.robinhood.com/chain/protocol-contracts)                                                 |
| `SignatureChecker` `0xbA3b…7DA6`        | **`0x`, absent.** Not a blocker: it is Circle's library, linked by FiatToken and placed only by the test harness and `infra/anvil/seed.sh`. `x402BatchSettlement`'s committed runtime does not reference it |

**Proof that the `x402BatchSettlement` difference is only the immutables.** All differing nibbles fall
in one region, offsets 16009 to 16097, holding:

- the chain id: `0x14a34` (84532, Base Sepolia) in the committed file, `0x1237` on mainnet, `0xb626` on
  testnet;
- a PUSH32 domain separator: `0xfa78e9a7…4a01` on mainnet and `0xa5fa2996…dfb9` on testnet.

Recomputing `EIP712Domain("x402 Batch Settlement", "1", chainId, 0x4020…0003)` gives exactly those two
separators, and `eip712Domain()` live returns that name, version, chain id 4663 and address.

**Live use on mainnet.** `eth_getLogs` on the contract over the last 200,000 blocks returned 90 logs:
18 each of `ChannelCreated`, `Deposited`, `Claimed`, `Refunded` and `Settled`. All 18 `ChannelCreated`
configs name USDG, and the one printed has `withdrawDelay` 900 s. Who ran them is unknown; the
uniform counts suggest a scripted run rather than organic demand (**Inference**).

**x402's own records do not list the chain.**

- [`contracts/evm/README.md`](https://github.com/x402-foundation/x402/blob/0c58a71cc2b050f8512e3226bfd0ff18a1fbe750/contracts/evm/README.md)'s
  batch-settlement deployment table has no Robinhood row, and
  [`network-and-token-support.mdx`](https://github.com/x402-foundation/x402/blob/0c58a71cc2b050f8512e3226bfd0ff18a1fbe750/docs/core-concepts/network-and-token-support.mdx)
  does not mention it.
- PRs adding USDG as the default asset are open, not merged:
  [#3770](https://github.com/x402-foundation/x402/pull/3770) and
  [#3761](https://github.com/x402-foundation/x402/pull/3761).
- Who deployed on mainnet is **unverified**: the explorer API is blocked. On testnet the explorer names
  the creator as `0x4e59…956C`, the deterministic CREATE2 deployer. Same address plus same code is what
  ADR 0075 relies on, and the deployer does not change that (**Inference**).

**Facilitators** (live `GET /supported`):

| Facilitator                                                                  | On `eip155:4663`                                           |
| ---------------------------------------------------------------------------- | ---------------------------------------------------------- |
| `api.solvador.com`                                                           | `batch-settlement`, `exact`, `upto`                        |
| `x402.dexter.cash`                                                           | `batch-settlement` (minimum about $0.005), `exact`, `upto` |
| `facilitator.bursar.world/x402`, `x402.t0ll.xyz`, `facilitator.priors.trade` | `exact` only, so no use for a channel                      |
| `x402.org/facilitator`                                                       | Not offered                                                |

Neither `batch-settlement` facilitator lists testnet 46630. Both name their own `receiverAuthorizer`.
Whether either relays a deposit toward a channel a TOON node would admit is **untested**. ADR 0076 has
the operator name the facilitator, and `facilitator_url` is optional in config.

### What a table would hold (Inference, from `crates/connector-config/src/settlement.rs`)

```toml
[settlement.eip155.4663]
rpc_url = "<a provider URL; the public one is rate-limited>"
token_address = "0x5fc5360D0400a0Fd4f2af552ADD042D716F1d168"
decimals = 6
asset_eip712_name = "Global Dollar"
asset_eip712_version = "1"
asset_transfer_method = "eip3009"

[settlement.eip155.4663.key]
key_file = "<path to settlement.key>"
```

Tracing the boot path by reading, each check's read succeeds live with the expected value: code at the
contract is non-empty, on-chain decimals match, the token's domain matches, and `authorizationState`
exists for `deposit_route`. **Not run.** ADR 0086's header still says "not yet built", but `settlement.rs`
at this commit parses these tables; the ADR's status should be read with that in mind. The asset id
would be `eip155:4663/erc20:0x5fc5360d0400a0fd4f2af552add042d716f1d168`.

### Possible frictions (all Inference, none tested)

- **Client decoding.** The backend uses `ethers`. Nitro blocks and receipts carry extra fields such as
  `l1BlockNumber`, and Nitro has its own transaction types. Reads of Arbitrum One have not been run on
  this path either, as far as this note found; the repository's Arbitrum mentions are config and
  identifier tests.
- **Polling.** `ConfirmPolicy` polls every 100 ms when direct, against a rate-limited public endpoint.
  A provider URL avoids this, as the docs advise anyway.
- **Log windows.** `MAX_BLOCK_RANGE` is 2,000 blocks, or about 200 s here, against far longer on Base.
  Live `eth_getLogs` over 200,000 blocks succeeded on the public RPC. Whether the watcher's chunking
  scales at 10 blocks per second was not traced.
- **State reads at a block.** `batch_settlement.rs` reads state at a receipt's block, which is recent,
  so the public RPC's pruning should not matter on that path.
- **Sanctions screening** can cause a transaction to be excluded. To this node's sender that may look
  like the "not observed" outcome `send.rs` already reports.

---

## 5. Local testing

All of this section is **Inference** against ADR 0084, except where it cites a source or a local run.

**Plain anvil with the placed bytecode is the fit.** It is what `X402Chain::place` already does for
every EVM network, and it meets ADR 0084's decision 7: it runs "the exact program the connector settles
against". §4 shows the deployed code is that program, apart from immutables that OpenZeppelin-style
EIP-712 caching recomputes when the chain id differs. x402's
[source](https://github.com/x402-foundation/x402/blob/0c58a71cc2b050f8512e3226bfd0ff18a1fbe750/contracts/evm/src/x402BatchSettlement.sol)
uses `block.timestamp` for withdrawal delays and never `block.number`, so Nitro's L1-estimate
`block.number` does not touch it. A grep of `crates/` found no use of ArbSys, ArbGasInfo or `l1BlockNumber`.
Anvil is about 0.03 s to spawn (ADR 0084), well inside its 10 s and 1 GB limits, and it stays per test.

**What anvil does not reproduce:**

- USDG itself. The harness deploys FiatToken v2.2, while USDG is a Paxos facet-routed token. Its domain
  has a different name and version, but the connector treats those as config. A USDG-specific tier-3 test
  would have to place the proxy, its facets and its storage. ADR 0084 decision 7 speaks of the settled
  program, and whether the token counts is a question for the owner (see Open questions).
- L1 data fees in gas estimation, the transaction filter, sub-second blocks, and Nitro's RPC shapes.

**Anvil fork.** Run locally with Foundry 1.7.1: `anvil --fork-url https://rpc.mainnet.chain.robinhood.com`.

- `eth_chainId` returned 4663.
- `x402BatchSettlement`'s code was readable, and `USDG.decimals()` returned 6.
- `ArbSys.arbBlockNumber()` failed with `InvalidFEOpcode`, because anvil has no Arbitrum precompiles.
  Its `--help` lists `ethereum`, `optimism` and `tempo` networks plus `--celo`, and no Arbitrum mode.

A fork is useful by hand, but cannot be the gate: it depends on a third party's RPC, the reason ADR
0084's decision 8 and ADR 0007 refuse a public network, and the public RPC drops historical state.

**Nitro dev node.** [`nitro-devnode`](https://github.com/OffchainLabs/nitro-devnode) runs one container,
`offchainlabs/nitro-node … --dev`, with a prefunded key
([script](https://github.com/OffchainLabs/nitro-devnode/blob/main/run-dev-node.sh)). So it fits decision
6's "one process or one container". It defaults to `v3.7.1-926f1ab`, older than the v3.12.2 mainnet
reports, though the tag can be overridden. The `v3.12.2-5f02f3f-amd64` image is about 1.4 GB
on [Docker Hub](https://hub.docker.com/r/offchainlabs/nitro-node/tags).
**Not measured:** startup time and memory, which decide per-test or shared under decision 2, and
whether it can run ArbOS 61. Also unknown: whether the clock can be warped. x402's withdrawal delay is at
least 15 minutes (`CONTRACT_MIN_WITHDRAW_DELAY_SECS`), so a chain without a warp would need decision 7's
"delay tests in bounded time" answered per chain. By contrast, Arbitrum's
[`nitro-testnode`](https://docs.arbitrum.io/run-arbitrum-node/run-local-full-chain-simulation) is a
docker compose set with a Geth parent chain and several Nitro roles, which decision 6 rules out.

**Public testnet.** Chain 46630 has the contracts and a Paxos USDG, so it can hold a manual check,
**off** the gate and only beside a tier-3 suite (decision 8).

**Suggested shape.** Admit under anvil like any other `eip155` network, then add one off-gate run on
46630 to close the §4 frictions. A Nitro dev-node suite is only worth measuring if that run finds
something anvil cannot show.

---

## 6. Permissioning and access

**Chain level**

- **Deployment.** Robinhood calls the chain "permissionless" ([overview](https://docs.robinhood.com/chain/),
  [ToS §2.1](https://docs.robinhood.com/chain/terms-of-service)). Its
  [deploy guide](https://docs.robinhood.com/chain/deploy-smart-contracts) needs only a wallet with ETH
  and mentions no allowlist or KYC.
- **KYC.** The ToS does not use the term. It does reserve the right to verify eligibility.
- **But blocking is reserved.** ToS §6 allows "restricting or blocking specific wallet addresses from
  participating on Robinhood Chain". §2.2 bars Prohibited Persons, including those in comprehensively
  sanctioned jurisdictions, and bars masking identity with a VPN or proxy. The terms name no list of
  restricted countries.

**Inference: that VPN clause may matter to an onion deployment.** A node using `rpc_via_socks_proxy`
reaches RPC through the `socks_proxy` (ADR 0073), and the ToS's anti-masking language could reach it.
This is a question for a lawyer, not a finding. The entity names are also inconsistent: §1 says RHDA,
LLC and §12.1 says Robinhood Digital Assets, LLC.

**Asset level** is §3: USDG freeze and pause, stock token blocklist, pause, admin burn and Reg S limits.

**Trust assumptions** ([L2BEAT](https://l2beat.com/scaling/projects/robinhood), read through the same
summarising tool):

- **Listing.** About $2.23B TVS. Categorised under "Others" and **not even Stage 0**: fewer than five
  external actors can challenge.
- **Upgrades.** A 7-of-8 multisig can upgrade core contracts instantly, so L2BEAT sees effectively no
  exit window. A 6-of-8 path sits behind a 7-day timelock.
- **Proofs.** Only whitelisted validators propose and challenge. Proposing opens to anyone after 28 days
  of inactivity. L2BEAT flags near-zero challenge stakes as a resource-exhaustion risk.
- **Sequencing.** Centralised, with one batch poster. Forced inclusion is available after at most 4 days,
  **but filtering defeats it**: a hash registered in `ArbFilteredTransactionsManager` fails even when
  force-included, and no delay applies to registering one. L2BEAT reports that list growing from 2 to
  about 6,228 entries since July 2026.
- **Node source.** Availability is "under review" and L2BEAT has not verified it. Robinhood's
  [notices](https://docs.robinhood.com/chain/notices-and-upgrades) say a source release follows the
  v3.11.4 image.
- **Liveness.** A 54-minute gap in state updates and a 12-minute data gap on Oct 29; 5 minutes on Oct 15.

**Where the sources disagree.** Robinhood [says](https://docs.robinhood.com/chain/governance) the chain
is governed by an eight-seat Security Council: Robinhood (2), BitGo, Chainlink Labs, Fireblocks, Offchain
Labs, Paxos, Talos. Routine actions need 6-of-8 with a 7-day timelock, and emergencies need 7-of-8 with
none. L2BEAT reads the same thresholds on chain but says the Security Council is "not properly set up",
presumably against its own criteria. Both can be true: the signers exist, and two of the eight are the
operator. L2BEAT's page, at the time read, also lists a zero-address EOA that it treats as a likely data
error, so its detail should not be read as flawless.

**Inference: what this means for a channel.** A voucher is only worth what can be landed. A filtered or blocked receiver holds
vouchers it cannot redeem, because redemption goes through a sequencer that can refuse the transaction. An instant upgrade can reach the rollup and bridge, but not
`x402BatchSettlement` itself, which is immutable; it could still change the chain's behaviour beneath it.

---

## Open questions

For the owner:

1. **Is a USDG-only leg wanted?** The connector's earnings are routing fees over peerings, so a leg
   only pays if peers and clients hold USDG on 4663. Reaching USDC elsewhere is a dealing boundary at a
   declared rate, with no rate source examined here.
2. **Is the trust model acceptable** given below-Stage-0 status, the transaction filter and instant
   upgrades?
3. **Does ADR 0084 decision 7's "program" include the token?** If so, USDG needs its own tier-3 placement
   rather than the FiatToken the harness uses today.

To check by running, none of which was done here:

4. Boot a connector on `[settlement.eip155.46630]` with testnet USDG and move a voucher through open,
   claim, settle and withdraw. Testnet's domain version was not checked; only its name and decimals were
   read.
5. Do `toon-client`, `rig` and `swap` accept a `Global Dollar` / `1` domain and 4663?
6. Measure Nitro dev-node startup and memory on the gate's runner, if it comes to that.

### Looked for and not found, or could not verify

- A primary source for bridged USDC on mainnet, and a Circle or Robinhood plan for native USDC or CCTP.
  Not found. The mainnet explorer API is behind Cloudflare, so a token search was not possible.
- Robinhood's Jul 1 mainnet post itself. The guessed URL returned 404 and Robinhood's Q2 results PDF
  returned 403, so the date rests on the newsroom listing and Arbitrum's factsheet.
- Who deployed x402 on mainnet and when. `eth_getCode` at older blocks fails on the public RPC, and
  the explorer API is blocked.
- Source for USDG's EIP-3009 and freeze facets, which are not verified on Sourcify. Behaviour is read
  from routing, reverts and live channels.
- Stock tokens' full jurisdiction list, linked from the docs and not opened. `Stock` contract terms at
  `robinhood.com/stocktoken/rhj` were not opened either.
- Paxos's freeze terms were read only through a search summary.
- Published rate limits for the public RPC, and Robinhood-documented faucet terms. None found.
- Opcode-level deviations beyond the docs' list, and the reorg behaviour of soft confirmations in practice.
- Native chain token: only a secondary mention.

---

## Appendix: live reads

All on 2026-11-02 at about 14:00 UTC. `M` is the mainnet RPC and `T` the testnet RPC.

| Read                                                                   | Result                                                                                             |
| ---------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------- |
| `eth_chainId` M / T                                                    | `0x1237` / `0xb626`                                                                                |
| `web3_clientVersion` M                                                 | `nitro/v3.12.2+5f02f3f-20261014T220458Z/linux-amd64/go1.25.12`                                     |
| `web3_clientVersion` T                                                 | `nitro/v3.12.3-rc.4+7116cfc-20261028T163253Z/linux-arm64/go1.25.12`                                |
| `eth_getCode` `0x4020…0003`, `…0004`, `…0005`, Permit2, M and T        | Non-empty                                                                                          |
| `eth_getCode` `0xbA3b…7DA6`, M and T                                   | `0x`                                                                                               |
| `ArbSys.arbOSVersion()` M                                              | 116. Nitro offsets this figure; docs and L2BEAT say ArbOS 61 (**Inference** that these agree)      |
| `cast block latest` M                                                  | `gasLimit` `0x4000000000000`, `baseFeePerGas` `0x133d4b0`                                          |
| `latest` / `safe` / `finalized` M                                      | 104271479 / 104266347 / 104262612                                                                  |
| `USDG.name/symbol/decimals` M                                          | "Global Dollar" / "USDG" / 6                                                                       |
| `USDG.DOMAIN_SEPARATOR()` M                                            | `0x7a3d7400b27830f4f91c2c16a082486d67c1befecaec2f53b33f1f35d5b62036`, equal to the recomputed hash |
| `USDG.version()` M                                                     | Reverts `0x800ab12c`                                                                               |
| `USDG.getFacet(0x88b7ab63)` M / T                                      | `0x780d30b6…1309` / `0x08f560a8…B811`                                                              |
| `USDG.getFacet(<isSanctioned>)` M, probing a guessed name              | Zero address, so no such selector                                                                  |
| `receiveWithAuthorization` (bytes), dummy signature, from the contract | Reverts `0x8baa579f`                                                                               |
| `eth_getLogs` on the contract, last 200,000 blocks, M                  | 90 logs, in blocks `0x6345847` to `0x636a1d5`                                                      |
| `eth_getCode` at block `0x6300000`, M                                  | `historical state … is not available`                                                              |
