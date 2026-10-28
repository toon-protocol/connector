# More than one EVM network per node: the facts the multi-EVM spec needs

> **Research note, not a decision record.** It gathers the facts that issue
> [#1566](https://github.com/toon-protocol/connector/issues/1566) (part of map
> [#1563](https://github.com/toon-protocol/connector/issues/1563)) needs for its next round. The owner
> has already decided two things (grilling session, 2026-10-28):
>
> - **Q1 is option (a).** There will be several EVM settlement tables keyed by chain id, and every
>   chain-scoped identifier (AssetId, journal channel key, client claim) gains the chain id. Only
>   `eip155` networks are covered. Hedera is **not** folded in; it stays parked and would get its own
>   table later.
> - **Q2.** A wire change under [ADR 0021](../adr/0021-vectors-are-normative-prose-is-not.md) is accepted.
>
> This note does not reopen either decision. Where a fact bears on how one is carried out, it says so.
> Each question below ends with a **Recommendation**, labelled as such. Every source was read on 2026-10-28.

**Evidence tags.** These follow [`x402-pending-chain-bindings.md`](x402-pending-chain-bindings.md)
(cited as **BINDINGS-NOTE**):

- CONFIRMED-CODE: read in source at a pinned commit, or read live from a chain's RPC.
- CONFIRMED-DOCS: read in the maintainer's own documents or specs.
- UNCONFIRMED: none of the above, and the entry says why.
- **Inference**: this note's own reasoning from confirmed facts, not a fact.

**Sources pinned.**

- **This repository**: `HEAD` `9a039468` (branch `adr-0077-zk-note`). Relative links are to that tree.
- **x402**: [`x402-foundation/x402@db2c98ec`](https://github.com/x402-foundation/x402/tree/db2c98ec9490e4e4286c862de845e667b037f219)
  (`main`, 2026-10-28T13:26Z). Cited as **X402**. BINDINGS-NOTE pinned `261d0c5e` on the same day.
- **toon-client**: [`toon-protocol/toon-client@6cfd3471`](https://github.com/toon-protocol/toon-client/tree/6cfd3471e7e23c65f3a96a3790a4779af341ae81)
  (`main`, last push 2026-10-23). Read only. Cited as **TC**.
- **CAIPs**: [`ChainAgnostic/CAIPs@f46c0fe0`](https://github.com/ChainAgnostic/CAIPs/tree/f46c0fe052eb80875dd94bbb39af16bb96ad6c1a)
  and [`ChainAgnostic/namespaces@463bae56`](https://github.com/ChainAgnostic/namespaces/tree/463bae5625aaa8c184ccc74f1db457da8a43e72b).
- **Circle**: [USDC contract addresses](https://developers.circle.com/stablecoins/usdc-contract-addresses).
  The page's `.md` rendering was fetched on 2026-10-28. It has no version pin, so line numbers below
  refer to that fetch. Cited as **CIRCLE**.
- **Live reads, 2026-10-28**:
  - `eth_chainId`, plus `eth_getCode` at `0x4020074e…0003`, on each chain's public RPC (§9);
  - USDC `name()` and `version()` on 13 chains (§9);
  - `GET https://x402.org/facilitator/supported` (§6).

---

## Verdict

**The brief's premise about channel ids is wrong, in the safe direction.** x402's `getChannelId` is
`_hashTypedDataV4(...)`, which hashes under the EIP-712 domain, and that domain includes `chainId`
(§1). So the same `ChannelConfig` gets a different channel id on every chain. Two consequences:

- **A cross-chain channel-id collision cannot happen** between networks with distinct chain ids.
- **A legacy `evm:0x…` journal entry can be assigned to its network by proof.** The entry carries
  the channel config, so recompute the channel id under each configured network's domain; at most
  one will match. Nothing has to be assumed.

**AssetId is the identifier that really collides.** The same token address exists on two chains in
practice:

- every anvil seeded by `infra/anvil/seed.sh` gets the same USDC address;
- Circle's USDC is `0x3600…0000` on both Arc mainnet and Arc testnet.

**The current schema version is 8, not 7.** ADR 0074 decision 3, as amended (#1429), moved it. So
this change takes `schema_version` from **8 to 9** (§2).
_Corrected 2026-10-28 (#1566): `main` had already moved it to 9 (#1537) when this note was pinned,
so ADR 0086 takes it from 9 to 10._

**x402 names a network as a CAIP-2 string, `eip155:<chainId>`, and nothing else.** That spelling is
used in three places:

- **x402's wire**: `accepted.network`;
- **TOON's own greeting and self-description**: `batchSettlements[].network` and `voucherSigners[].network`;
- **TOON's persisted runtime peer table**, which already writes `network` on every `evm-voucher` row (§4).

TC already parses `network` and chooses terms by it. The cheapest consistent choice is therefore
CAIP-2 everywhere a network is named. The work is in the places that key by `SettlementChain::Evm`
as a singleton: 86 lines in 14 non-test files (§11).

**Fourteen `eip155` networks carry `x402BatchSettlement` today.** Each was confirmed by `eth_getCode`
on 2026-10-28, and each has Circle-issued USDC at an address Circle lists (§9):

- **Mainnets (10):** Ethereum, Base, Arbitrum, Optimism, Polygon, Avalanche, Celo, Linea, Unichain
  and World Chain;
- **Testnets (4):** Base Sepolia, Arbitrum Sepolia, Ethereum Sepolia and Avalanche Fuji.

Twelve chains in x402's README list also carry it, including Monad and Monad Testnet. The README
**omits** Base Sepolia, the network the devnet runs on, although the contract is there.

**USDC's EIP-712 name differs by network.** It is `"USD Coin"` on Base and `"USDC"` on Base Sepolia,
Celo, Linea, Unichain, World Chain and Monad. So `asset_eip712_name` must be set per table (§5, §9).

---

## 1. The premise: what an EVM channel id binds

- **`getChannelId` is chain-bound.** `x402BatchSettlement.getChannelId(config)` returns
  `_hashTypedDataV4(keccak256(abi.encode(CHANNEL_CONFIG_TYPEHASH, config)))`
  ([X402 `x402BatchSettlement.sol` L442–446](https://github.com/x402-foundation/x402/blob/db2c98ec9490e4e4286c862de845e667b037f219/contracts/evm/src/x402BatchSettlement.sol#L442-L446)).
  The domain is `EIP712("x402 Batch Settlement", "1")` (L185). CONFIRMED-CODE.
- **The spec says so in words.** `channelId = EIP712Hash(ChannelConfig)` "binds the immutable config
  to the EVM `chainId` and deployed `x402BatchSettlement` address, so the same config produces
  different IDs across chains or deployments"
  ([X402 `scheme_batch_settlement_evm.md` L33](https://github.com/x402-foundation/x402/blob/db2c98ec9490e4e4286c862de845e667b037f219/specs/schemes/batch-settlement/scheme_batch_settlement_evm.md#L33)).
  Verification rule 1 checks "the config's chain-bound EIP-712 hash" (L614). CONFIRMED-DOCS.
- **This repository does the same.**
  [`connector_signer::evm_batch_channel_id`](../../crates/connector-signer/src/voucher_signature.rs)
  hashes the seven fields and then calls `domain.hash_typed_data(...)` (L178–192). That function's
  separator includes `chain_id` (L139–147). `BatchSettlementDomain { chain_id, verifying_contract }`
  is built by `BatchSettlementDomain::x402(chain_id)` (L125–137). CONFIRMED-CODE.
  - The brief said "`getChannelId` hashes only the seven fields (no chainId)". That is true of the
    struct hash only, not of the id.
- **What this means for collisions.** **Inference.**
  - Two networks with **different** chain ids cannot produce the same channel id except by a keccak
    collision.
  - Two networks with the **same** chain id produce identical ids, and identical voucher digests, for
    identical configs. Examples: two anvils both left at 31337, or a fork of a live chain. Boot must
    therefore refuse two tables that report the same `eth_chainId` (§8).

## 2. How a client claim names its network (next-round Q1)

**What x402 puts on the wire.** CONFIRMED-DOCS / CONFIRMED-CODE.

- **The network field.** A v2 `PaymentPayload` carries the network only inside
  `accepted: PaymentRequirements`, as `network`, "Blockchain network identifier in CAIP-2 format (e.g.,
  "eip155:84532")" ([X402 `x402-specification-v2.md` L125, L166–168, L201](https://github.com/x402-foundation/x402/blob/db2c98ec9490e4e4286c862de845e667b037f219/specs/x402-specification-v2.md#L125)).
  The scheme-specific `payload` (for EVM `batch-settlement`, the `channelConfig`, `voucher` and
  `deposit` objects) carries **no** network field
  ([`scheme_batch_settlement_evm.md` L159–200](https://github.com/x402-foundation/x402/blob/db2c98ec9490e4e4286c862de845e667b037f219/specs/schemes/batch-settlement/scheme_batch_settlement_evm.md#L159-L200)).
- **A mismatch has its own error.** `invalid_batch_settlement_evm_network_mismatch`: "Payment payload
  `accepted.network` does not match `paymentRequirements.network`" (L791).
- **The SDK types agree.**
  - **Go**: `PaymentPayload{ Accepted PaymentRequirements }`, and `GetNetwork()` returns
    `p.Accepted.Network`. `checkNetworkV2` refuses anything that is not CAIP-2
    ([`go/types/v2.go` L14–41](https://github.com/x402-foundation/x402/blob/db2c98ec9490e4e4286c862de845e667b037f219/go/types/v2.go#L14-L41)).
  - **TypeScript**: `PaymentRequirements.network: Network` and `PaymentPayload.accepted`
    ([`typescript/packages/core/src/types/payments.ts` L12–14, L30–33](https://github.com/x402-foundation/x402/blob/db2c98ec9490e4e4286c862de845e667b037f219/typescript/packages/core/src/types/payments.ts#L12-L33)).
  - **Python**: `network: NetworkV2`, validated as CAIP-2, and `PaymentPayload.accepted`
    ([`python/x402/schemas/payments.py` L43–57, L91–104](https://github.com/x402-foundation/x402/blob/db2c98ec9490e4e4286c862de845e667b037f219/python/x402/schemas/payments.py#L43-L104)).
- **Bare chain ids appear only in legacy names.** The `/supported` response and §11.1 use CAIP-2
  throughout (L443–493, L633–648). A facilitator's `signers` map is keyed by CAIP-2 **patterns**
  (`eip155:*`). v1's bare names (`base-sepolia`) survive only as legacy entries (§6's live read).

**What TOON's claim looks like today.** CONFIRMED-CODE.

- **The parser.**
  [`parse_client_claim`](../../crates/connector-domain/src/client_claim.rs) dispatches on `blockchain`
  `"evm"` or `"solana"` (L640–656). An EVM voucher's fields are `channelId`, `maxClaimableAmount`,
  `signature` and an optional `channelConfig` (L568–592). There is no network field.
  - **Unknown keys are tolerated**: nothing refuses an extra field.
- **The vectors.** [`vectors/wire-vectors.json`](../../vectors/wire-vectors.json) is at
  `schema_version` **8** (L2), and [`connector-vectors/src/lib.rs`](../../crates/connector-vectors/src/lib.rs)
  L181 defines `SCHEMA_VERSION: u32 = 8`. Its doc comment records 7 (#1384) and 8 (#1429).
- **Every EVM group carries `"blockchain":"evm"` and no network**, while the fixture's domain is
  `chain_id: 84532`. The groups are `claim_voucher` (L516), `peer_carriage.voucher_evm` (L164),
  `voucher_claim_state_challenge` (L620), `payout_voucher` (L736) and the BTP
  `client_auth_channel_challenge` (L770).
- **The retired `toon-channel` claim carried `"chainId":84532`, a bare number**. It is kept only in
  the refused vectors (L682, L700).
- **TC emits the same shape.** `evmVoucherClaim` writes `blockchain: 'evm'` with no network
  ([TC `packages/client/src/channel/batch-settlement/claim.ts` L31–55](https://github.com/toon-protocol/toon-client/blob/6cfd3471e7e23c65f3a96a3790a4779af341ae81/packages/client/src/channel/batch-settlement/claim.ts#L31-L55)).
- **TC already reads CAIP-2 and chooses terms by it.**
  - `parseBatchSettlementTerms` reads `network` and branches on `network.startsWith('eip155:')`
    ([TC `offers.ts` L82–115](https://github.com/toon-protocol/toon-client/blob/6cfd3471e7e23c65f3a96a3790a4779af341ae81/packages/client/src/channel/batch-settlement/offers.ts#L82-L115)).
  - `chooseBatchSettlement(desc, chain, network?)` already filters by network (L167–178).

**What the receiver could infer without a field.** **Inference** from §1.

- **A first voucher is self-identifying.** It carries `channelConfig`, so the chain is whichever
  configured domain reproduces `channelId`. TC sends `channelConfig` on **every** voucher
  (`claim.ts` L24–29).
- **These cannot be inferred:** a later voucher without a config, the claim-state challenge, the BTP
  auth challenge, and a **Solana** network. Their digests depend on a domain the verifier has to pick
  first.
- **The verdict:** inference is enough to check an explicit field, not to replace one.

**Mixed versions.** **Inference.**

- **A new client against an old node.** The client sends `network` to a single-EVM node, which ignores
  the unknown key. If the network is not the node's, recomputing the channel id under the node's own
  domain fails (§1), so the claim is refused rather than mis-credited. This fails safe.
- **An old client against a new node.** If `network` is required, the old claim is refused as
  malformed. A named refusal would be kinder, for the same reason ADR 0075 refuses `toon-channel` by
  name.

**Recommendation.** Add a **`network` field holding a CAIP-2 string** (`eip155:8453`) to every EVM
claim, challenge and payout voucher.

- **Make it required for `blockchain: "evm"`, and refuse it by name when absent.** Check it against
  the domain the channel id recomputes under.
- **Keep `blockchain`.** It still picks the parser.
- **Reject the alternatives:**
  - a bare numeric `chainId` repeats the retired `toon-channel` shape and is not what x402 or TC uses;
  - a new `blockchain` spelling (`"eip155"`) breaks the `blockchain` → parser mapping that Solana
    shares.
- **Solana.** Decide in the same bump whether it gets `network` too (§12, D2). It is symmetric and
  cheap.
- **Bump `schema_version` to 9** and add a mismatch vector.

## 3. AssetId spelling (next-round Q2)

**Today.** CONFIRMED-CODE, [`connector-domain/src/asset.rs`](../../crates/connector-domain/src/asset.rs).

- **The shape.** `AssetId { chain: AssetChain, token }` (L125–128). `AssetChain` is `Evm | Solana`
  (L77–82). `Display` writes `<chain>:<token>`, so `evm:0x…` or `solana:<mint>` (L171–177).
- **Parsing.** `FromStr` splits at the **first** `:` (L179–196). An EVM token with an address's shape
  is canonicalised to lowercase (L247–254); anything else is kept as written.
- **No chain id.** The doc comment (L13–17) says the identity is "a chain and a contract, and nothing
  else".

**Where AssetId is parsed, printed or persisted.** CONFIRMED-CODE.

- **Config.**
  - Parsed in `[[tokens]] asset`, `[[rates]] from/to` and each quote leg's `quote_token`
    ([`denomination.rs`](../../crates/connector-config/src/denomination.rs) L19–60).
  - Built from each settlement table's `token_address` for `ClientChannelAssets` and
    `PeeringAssets`.
  - Users: [`client_channel_asset.rs`](../../crates/connector-config/src/client_channel_asset.rs),
    [`peering_asset.rs`](../../crates/connector-config/src/peering_asset.rs) and `settlement.rs`.
- **Runtime.**
  - It keys the rate table: `connector-domain/src/rate_table.rs`, `connector-runtime/src/rate_table.rs`
    and `rate_poller.rs`.
  - It keys the `RateSource` port and its EVM implementation, which accepts only
    `asset.chain() == AssetChain::Evm` (`connector-rate-source-evm/src/lib.rs` L457).
- **The operator surface.** `GET /rates` serialises `from` and `to` in the text form
  ([`operator_view.rs`](../../crates/connector-runtime/src/operator_view.rs) L213–306).
- **Never persisted.** There is nothing in `state_dir`: rates are runtime state (ADR 0071 decision 6).
- **Never on the wire.** It is not in the greeting, the self-description or any vector. ADR 0071
  decision 7 keeps the wire "unit-silent". The greeting's `asset` is a bare address beside `network`
  ([`runtime.rs`](../../crates/connector-cli/src/runtime.rs) L913–927).

**Committed spellings a rename breaks.** CONFIRMED-CODE.

- **`evm:0x…` as an AssetId** is in exactly one committed config:
  [`local/dealing/connector-b.toml`](../../local/dealing/connector-b.toml) L103 (`[[tokens]]`) and L125
  (`[[rates]] from`).
  - It also appears in prose in [`local/README.md`](../../local/README.md),
    [`docs/guide/local-peering-lab.md`](../guide/local-peering-lab.md), `docs/protocol/client-edge-spec.md`
    and `local/rehearsal.sh` L44, where it is the channel-key spelling.
  - It also appears in 4 integration-test files under `crates/connector-bin/tests/`.
  - The devnet configs declare no `[[tokens]]`.
- **`evm:<chainId>` already exists, outside the connector.**
  - [`infra/linode-relay/swap.config.json`](../../infra/linode-relay/swap.config.json) L5, L13 and L30
    use `"chain": "evm:84532"` (the `swap` maker's own format).
  - [`deploy/connector-rust/connector.toml`](../../deploy/connector-rust/connector.toml) L151 has a
    commented, app-opaque example `chain = ["evm:84532"]`.

**CAIP-19.** CONFIRMED-DOCS.

- **The syntax.** `asset_type = chain_id "/" asset_namespace ":" asset_reference`, with namespace
  `[-a-z0-9]{3,8}` and reference `[-.%a-zA-Z0-9]{1,128}`. It is case-sensitive, and **`:` and `/` are
  not allowed in the reference**
  ([CAIP-19 L44–56](https://github.com/ChainAgnostic/CAIPs/blob/f46c0fe052eb80875dd94bbb39af16bb96ad6c1a/CAIPs/caip-19.md#L44-L56)).
  Its status is **Review**, not Final (L6). CAIP-2 is Final.
- **The ERC-20 profile.** `eip155:1/erc20:0x6b17…` ([CAIP-19 L162](https://github.com/ChainAgnostic/CAIPs/blob/f46c0fe052eb80875dd94bbb39af16bb96ad6c1a/CAIPs/caip-19.md#L162);
  [eip155 `caip19.md` L35–36](https://github.com/ChainAgnostic/namespaces/blob/463bae5625aaa8c184ccc74f1db457da8a43e72b/eip155/caip19.md#L35-L36)).
- **The Solana profile.** `solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp/token:EPjF…`, which includes the
  cluster's genesis prefix ([solana `caip19.md` L44](https://github.com/ChainAgnostic/namespaces/blob/463bae5625aaa8c184ccc74f1db457da8a43e72b/solana/caip19.md#L44)).

**How today's parser reads each candidate.** **Inference** from `FromStr` (L179–196).

| Candidate                    | What a build at `9a039468` does with it                                                                                                         |
| ---------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| `eip155:8453/erc20:0x8335…`  | `AssetChain::from_str("eip155")` gives `UnknownChain`. **Refused by name**: an old binary fails loudly.                                         |
| `evm:8453:0x8335…`           | Chain `evm`, token `8453:0x8335…`, kept uncanonicalised. **Parses as a different token**. The refusal comes only later, when no table holds it. |
| `eip155:8453:0x8335…` (flat) | `UnknownChain("eip155")`. Refused by name.                                                                                                      |

**Recommendation.** Use the **CAIP-19 spelling for EVM**, `eip155:<chainId>/erc20:<0x lowercase>`.

- **It nests the CAIP-2 network unchanged.** The same `eip155:8453` prefix is then the network in
  claims, greeting, self-description, peer table and AssetId.
- **An old binary refuses it by name**, while `evm:<id>:0x…` it misparses.
- **It is already a standard.**
- **Make it the only accepted form.** The new parser should refuse `evm:0x…` by name ("an EVM asset
  now names its chain: `eip155:<chainId>/erc20:<address>`") rather than guess a chain. The one
  committed fixture and the test literals are rewritten in the same change.
- **Solana.** Whether it moves to `solana:<genesis>/token:<mint>` in the same change is a separate
  call (§12, D2). Leaving `solana:<mint>` alone is coherent while a node has one Solana table.

## 4. Journal channel key and persisted state (next-round Q3)

**What `state_dir` holds** ([`runtime.rs`](../../crates/connector-cli/src/runtime.rs) L588–601). CONFIRMED-CODE.

| File                            | Chain-keyed content                                                                                                                                                      | Network recorded?                                                                                                                             |
| ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------- |
| `client-edge-claims.log`        | `BatchChannelAdmitted { channel_id, presentation }`, `InboundClaimAccepted` and the watermark entries, all keyed `evm:0x<64>` / `solana:<b58>` (`canonical_channel_key`) | **No.** The EVM presentation is a fixed 140 bytes: 5 addresses, `withdrawDelay` and `salt` (`client-edge/src/batch_settlement.rs` L132, L177) |
| `outbound-channels.log`         | `OutboundChannelOpening { channel_id, record }` with `OutboundChannelRecord::encode`, `RECORD_VERSION = 1` (`port.rs` L377–478)                                          | **No.** The EVM record is the channel id, the 7 config fields and the deposit                                                                 |
| `runtime-peers.json`            | `RuntimePeerChannel::EvmVoucher { outbound_channel_id, voucher_signer, network }` (`peer_route_store.rs` L146–160)                                                       | **Yes**: `network`, CAIP-2 `eip155:<chainId>`                                                                                                 |
| `controller-day-totals.journal` | Controller day totals                                                                                                                                                    | Not chain-keyed                                                                                                                               |

- **Decoding is strict.** `OutboundChannelRecord::decode` refuses any other version byte and any
  trailing bytes (`port.rs` L423–474; test L1059–1061). A version-2 record that adds a chain id
  cannot be misread by this build, and the reverse needs explicit handling.
- **App-facing headers carry the key.** `X-TOON-Payer` carries the channel key and `X-TOON-Chain`
  its namespace (`evm`), both to the app
  ([`attribution.rs`](../../crates/connector-runtime/src/attribution.rs) L41–57;
  [`client-edge-spec.md`](../protocol/client-edge-spec.md) L908–910). A key change is therefore also
  an **app-facing** change under ADR 0040.

**Can a cross-chain channel id collide?**

- **Not across distinct chain ids** (§1). CONFIRMED-CODE.
- **Same token address on two chains.** It happens, but cannot cause a channel-id collision.
  - Circle's USDC is `0x3600000000000000000000000000000000000000` on both **Arc** mainnet and Arc
    testnet (CIRCLE L43, L103). CONFIRMED-DOCS.
  - Every anvil seeded by [`infra/anvil/seed.sh`](../../infra/anvil/seed.sh) deploys FiatToken at
    `0xe7f1725E…0512` by deployer nonce (L49, L82). CONFIRMED-CODE.
  - Among the batch-settlement networks of §9, Circle's mainnet USDC addresses are all distinct.
  - Either way it is an **AssetId** problem, not a channel-key problem.
- **The only real collision is the same chain id twice.** Two tables pointing at chains that report
  the same `eth_chainId` produce identical ids. The local stack starts anvil at `--chain-id 31337`
  ([`infra/anvil/entrypoint.sh`](../../infra/anvil/entrypoint.sh) L15). **Inference.**

**Migration.**

- **Precedent.** ADR 0075 decision 8 refuses a journal holding `toon-channel` entries **at boot, by
  name**, and never skips them, because "a skipped entry is a claim somebody could still redeem that
  this node has forgotten it signed" ([ADR 0075 L271–274](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md)).
  It is implemented in `refuse_toon_channel_journals` (`runtime.rs` L561). CONFIRMED-DOCS / CONFIRMED-CODE.
- **This case differs.** A `toon-channel` entry had no successor scheme. A legacy `evm:0x…` entry is
  still a live x402 channel that the new build can settle. **Inference.**
- **Its network can be proved, not assumed.** Both `BatchChannelAdmitted.presentation` and the
  version-1 `OutboundChannelRecord` hold the full `ChannelConfig`. Recompute the id under each
  configured table's domain:
  - exactly one match assigns the entry;
  - no match means the operator removed that network while the node still holds value on it, and
    calls for refusal by name. **Inference.**
  - `InboundClaimAccepted`/watermark entries keyed `evm:<id>` inherit the network of the
    `BatchChannelAdmitted` with that id.

**Recommendation.**

- **Spell the key `eip155:<chainId>:0x<64 hex>`**, the CAIP-2 network plus the id, if the key gains
  the chain id as decided.
- **Write `OutboundChannelRecord` version 2** with the chain id.
- **Read legacy version-1 records and `evm:0x…` entries at boot and assign them by recomputation**,
  refusing by name only when no configured network or more than one claims an entry. That gives an
  **in-place upgrade**, not a drain.
- **Flag to the owner:** §1 makes the channel id itself globally unique. Keeping the key `evm:0x…`
  and recording the network beside it would avoid both the migration and the `X-TOON-Payer` change
  (§12, D1).

## 5. Config shape (next-round Q4)

**Today.** CONFIRMED-CODE, [`connector-config/src/settlement.rs`](../../crates/connector-config/src/settlement.rs).

- **The section.** `RawKeyedSettlementConfig { evm: Option<…>, solana: Option<…> }`, with
  `deny_unknown_fields` (L46–58). An unknown table such as `[settlement.slana]`, or one named by chain
  id, is a load error.
- **The legacy flat shape** (`chain = …` under `[settlement]`) is recognised only to be refused by
  name (L16–42, L634).
- **The EVM table's keys** (L71–102): `rpc_url`, `token_address`, `decimals`, `key`,
  `rpc_via_socks_proxy`, `min_withdraw_delay_secs`, `asset_eip712_name`, `asset_eip712_version`,
  `asset_transfer_method` and `facilitator_url`.
  - These are refused by name: `contract_address`, `channel_index_from_block`,
    `channel_index_confirmations` and `batch_settlement`.
  - There is **no `chain_id` key**. The chain id is read from the RPC
    (`connector-settlement-evm/src/lib.rs` L63).
- **`SettlementChain::name()` is reused as the operator surface's `chain` field** (L150–163).
  `POST /channels` and `POST /peers`'s optional chain filter both use it.

**Precedent for arrays of tables.** CONFIRMED-CODE.

- `[[tokens]]`, `[[rates]]`, `[[peers]]`, `[[routes]]`, `[[peer_channels]]` and `[[pay_channels]]`
  are all `Vec<Raw…>` ([`config.rs`](../../crates/connector-config/src/config.rs) L44, L129, L135,
  L142, L242).
- **`denomination.rs` L19–28 states the house rule**: a settlement table "is keyed by chain because
  there is exactly one per chain and the chain's own name is the natural key"; `[[tokens]]` is an
  array because "`evm:0x833589...` is the value, not a heading". With several EVM networks, a chain id
  is again a natural key. **Inference.**

**A trap in the channel rows.** CONFIRMED-CODE.

- `[[peer_channels]]` and `[[pay_channels]]` **already parse `chain_id` to refuse it by name**, as a
  retired TOON-era key ([`peer_channel.rs`](../../crates/connector-config/src/peer_channel.rs) L57–75,
  L82; [`pay_channel.rs`](../../crates/connector-config/src/pay_channel.rs) L62–79, L86).
- **Each row infers its chain from the spelling of its `voucher_signer` or `outbound_channel`**: `0x`
  means EVM, base58 means Solana (`peer_channel.rs` L92–112; `pay_channel.rs` L97–119). With several
  EVM tables that inference is ambiguous.
- **The trap.** These rows need a network key, and reusing `chain_id` there would turn a
  refused-by-name key back into a live one. ADR 0009's rule for a removed key forbids exactly that.
  **Inference.**

**Declared versus read chain id.** CONFIRMED-CODE.

- **The precedent.** `[announce].solana_chain_id` "was a second declaration of a fact
  `[settlement.solana]` already holds -- which is how a mainnet node came to describe itself as
  devnet (issue #981)" ([`error.rs`](../../crates/connector-config/src/error.rs) L1109–1118;
  `runtime.rs` L1586–1587).
- **The connector's answer to #981 was "derive, don't declare".** A table keyed by chain id
  necessarily declares it. The safe form is **declare-and-assert**, as `decimals` already is: a
  boot-time assertion against the chain, never an input (ADR 0071 decision 4). **Inference.**

**SOCKS circuit pinning.** CONFIRMED-CODE / CONFIRMED-DOCS.

- `Circuit` is a two-valued enum, `EvmSettlement | SolanaSettlement`, with fixed SOCKS usernames
  `toon-settlement-evm` and `toon-settlement-solana`
  ([`connector-chain-rpc/src/lib.rs`](../../crates/connector-chain-rpc/src/lib.rs) L82–118).
- `settlement_transports` maps each table's `SettlementChain` to its `Circuit` (`runtime.rs`
  L1443–1484), and `SettlementTransports` has one `evm` slot (L1421–1424).
- ADR 0073 decision 3 pins "**one pinned circuit per chain**" so that "no single exit sees both
  chains' RPCs" ([ADR 0073 L160, L185, L217](../adr/0073-settlement-rpc-may-ride-the-circuit-once-every-wait-on-it-is-bounded.md)).
- **Consequence.** Two EVM tables built on today's mapping would **share** the `toon-settlement-evm`
  circuit. One exit would then see both networks' RPCs, which is the linkage decision 3 exists to
  prevent. **Inference.**

**Recommendation.**

- **Use keyed tables with the network in the table key**: `[settlement.eip155.<chainId>]`, e.g.
  `[settlement.eip155.8453]`.
  - It follows the house rule that a table is keyed by its natural key.
  - It reads as the CAIP-2 id.
  - It gives `deny_unknown_fields` a place to catch typos per table.
  - It keeps `[settlement.solana]` as it is.
- **Refuse `[settlement.evm]` by name**, with the rewrite in the message.
- **At boot, assert that `eth_chainId` equals the key**, refuse a mismatch by name, and refuse two
  tables whose chains report the same id.
- **Give each EVM table its own circuit username**, e.g. `toon-settlement-eip155-<chainId>`.
- **Name the network on the channel rows with a new key** (`network = "eip155:8453"`), never by
  reviving `chain_id`.
- **Notice of breakage.** This is a breaking config change for every node repo that bind-mounts a
  `[settlement.evm]` TOML (CLAUDE.md, "Environments").

## 6. Settlement key per network (next-round Q5)

- **ADR 0075 decision 3 makes "the chain's settlement key" the voucher signer** on each chain, with
  `payerAuthorizer == payer` on EVM ([ADR 0075 L122–140](../adr/0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md)).
  It speaks per **chain**, and "chain" meant one EVM network when it was written. CONFIRMED-DOCS.
- **The code builds a wallet per backend.** It reads `eth_chainId` and calls
  `wallet.with_chain_id(chain_id)` (`connector-settlement-evm/src/lib.rs` L54–75). Each backend's
  `Sender` holds one lock across nonce choice, signing and sending, seeded from `pending`
  (`send.rs` L1–50). CONFIRMED-CODE.
- **Sharing a key across networks.** Nonces are per chain, so one key on N networks means N
  independent nonce sequences and no conflict. EIP-712 domains (§1) and EIP-155 transaction signing
  both bind the chain id, so nothing replays across networks. **Inference.**
  - The hazard is two tables on the **same** chain id with the same key: two `Sender`s racing one
    nonce sequence. §5's duplicate-chain-id refusal covers it.
- **The published voucher signer** is one `VoucherSignerFact { network, signer }` per table
  (`runtime.rs` L906–909). A shared key publishes the same address under each network. POST /peers
  already matches signer to terms **by network** (`peering.rs` L534–540). CONFIRMED-CODE.
- **x402 facilitators do the same.** They publish **one** EVM signer set for all networks
  (`"eip155:*"`; spec L485; live `x402.org` `/supported`, §7). CONFIRMED-DOCS.

**Recommendation.** **Make `key` per table**, keeping the code's shape where every table is
self-contained, and let the operator point several tables at the same file. Do not add a shared
`[settlement.eip155.key]`: it would be a second place a key can be declared. If an operator wants
keys separated per network, the per-table form already allows it.

## 7. Facilitator per network (next-round Q6)

- **Built today:** `[settlement.evm] facilitator_url` and `asset_transfer_method`, published as
  `facilitator` and `assetTransferMethod` in the EVM entry's `extra`, in both the greeting's
  `accepts[]` and the self-description's `batchSettlements`. The connector never calls the
  facilitator ([ADR 0076 L52–67](../adr/0076-the-operator-names-the-facilitator-and-pays-its-gas.md);
  `runtime.rs` L913–927). CONFIRMED-DOCS / CONFIRMED-CODE.
- **One facilitator serves several networks.** `/supported` lists `kinds[]`, each with its own
  `network` (spec L443–493).
  - x402.org's live `/supported` lists `batch-settlement` only on `eip155:84532`, plus `exact` and
    `upto` there. Its signers are keyed `eip155:*`, with one address. CONFIRMED-CODE (live).
- **The greeting already has one `accepts[]` entry per element of `batch_settlements`**, a `Vec`
  (`runtime.rs` L881–959). N EVM tables would give N `eip155:*` entries with no structural change.
  TC's `chooseBatchSettlement(chain, network?)` picks among them (§2). CONFIRMED-CODE.
- **`asset_transfer_method` depends on the token, so it varies by network.**
  - USDC on these chains is FiatToken with ERC-3009.
  - A table settling a non-USDC token needs `permit2`.
  - The EIP-712 name differs per network (§9). **Inference** from the live reads.

**Recommendation.** **Make all of them per table:** `facilitator_url`, `asset_transfer_method`,
`asset_eip712_name` and `asset_eip712_version`.

- Each is a fact about one network's token, or about who sponsors deposits **there**.
- Operators who use one multi-network facilitator write the same URL in each table.
- `facilitator` stays inside each `accepts[]` entry's `extra`, where TC already reads it.

## 8. Peerings, routes and dealing (next-round Q7)

**How a peering binds to a chain today.** CONFIRMED-CODE.

- **`POST /peers`** reads the peer's self-description and narrows `batchSettlements` with
  `shared_settlement_of(document, pays_on, wanted: Option<SettlementChain>, url)`
  ([`peering.rs`](../../crates/connector-runtime/src/peering.rs) L522–565).
  - The result is `NoSharedChain`, or **`AmbiguousChain`** when more than one entry survives.
  - It then refuses `NetworkMismatch` if the peer's network differs from `self.x402_network(chain)`
    (L337–347, L186–192).
  - `x402_network` is a `Vec<(SettlementChain, String)>` searched by chain
    ([`connector.rs`](../../crates/connector-runtime/src/connector.rs) L1252–1282). That is one
    network per chain.
- **What breaks.**
  - A peer that publishes Base and Arbitrum, peered from a node on only Arbitrum, yields two `evm`
    entries before the network check, so `AmbiguousChain`.
  - With one entry on the "wrong" network, `NetworkMismatch` fires instead of the right one being
    selected. **Inference.**
- **The durable record already names the network.** `runtime-peers.json`'s `evm-voucher` row has
  `network` (§4).
- **Config peerings carry no network.** `[[peer_channels]]` and `[[pay_channels]]` infer the chain
  from spelling (§5).
- **Carriage is chain-blind.** `PeerCarriage` (BTP or ILP-over-HTTP) carries no chain (ADR 0075
  decision 4, "Carriage is unchanged").
- **Per-network state is cleanly separable.** A peering is "two channels", and each channel is on one
  network by its domain. **Inference:** nothing stops a peering from paying on Base and being paid on
  Arbitrum. Whether to allow that is a decision (§12, D4).
- **`X-TOON-Chain`** is the channel key's namespace, `evm` or `solana`
  ([`attribution.rs`](../../crates/connector-runtime/src/attribution.rs) L55–57). With several EVM
  networks, an app can no longer tell from it which network, or which token's base units,
  `X-TOON-Amount` is in. **Inference.**

**Is Base USDC → Arbitrum USDC a dealing boundary?**

- **The ADR's wording.** ADR 0071 decision 2 says a "**same-asset** pair (USDC on Base to USDC on
  Solana — today's mixed-chain topology) has no rate row by definition and forwards unconverted"
  ([ADR 0071 L88–93](../adr/0071-a-forward-crosses-a-denomination-at-a-declared-rate.md)).
  CONFIRMED-DOCS.
- **The code's rule.**
  - The boundary is plain `AssetId` inequality: `crosses` returns `(incoming != outgoing).then_some(…)`
    ([`peering_asset.rs`](../../crates/connector-config/src/peering_asset.rs) L124–129).
  - **A node that declares no `[[tokens]]` resolves nothing and never converts** (L11–17). That is how
    `mixed-chain` forwards Base→Solana USDC unconverted.
  - A node that **does** declare tokens treats `evm:0x…` → `solana:…` as a boundary needing a rate,
    by the config test `two_peerings_on_two_chains_are_an_ordered_pair_of_tokens`
    ([`config.rs`](../../crates/connector-config/src/config.rs) L2534–2566).
  - CONFIRMED-CODE.
- **The same rule applies to EVM↔EVM.** With chain-scoped AssetIds, Base-USDC and Arbitrum-USDC are
  different AssetIds. A non-dealing node forwards them unconverted; a dealing node needs a rate row.
  **Inference.**
- **The rate source cannot quote a cross-network pair.**
  - A quote leg must be on the token's own chain (`TokenQuoteOffChain`, `denomination.rs` L523–528),
    and the last leg must **equal** the numeraire AssetId (L552–557).
  - There is **one** numeraire per node (ADR 0071 decision 3).
  - Today both checks compare `AssetChain::Evm` with itself. Once the chain id is in the AssetId,
    only tokens on the **numeraire's own network** can self-source by TWAP. Every other network's
    tokens, the same USDC included, need static `[[rates]]` rows. **Inference**, from code.
- **Rate sources are keyed by chain, not network.** `rate_sources` registers one
  `UniswapV3RateSource` on the one EVM transport, keyed `AssetChain::Evm` (`runtime.rs` L1397–1409).
  CONFIRMED-CODE.

**Recommendation.**

- **Peering.** Change `POST /peers`'s `chain` filter to a CAIP-2 `network`, and select the entry whose
  network the node has a table for. Make `x402_network` a lookup by network. Add `network` to the
  `[[peer_channels]]` and `[[pay_channels]]` rows (§5).
- **Leave dealing as the code has it.** Distinct AssetIds are a boundary for a dealing node, with a
  static 1:1 row as the operator's explicit statement that two USDCs are worth the same. This is
  "never silently" applied honestly.
- **Amend ADR 0071 decision 2's "same-asset" sentence** to say what the code does.
- **Key `RateSources` by network**, one source per EVM table.

## 9. Boot checks and tests (next-round Q8)

- **The boot refusal is per chain today.** `RuntimeError::X402NotDeployed { table: &'static str, … }`
  carries the static `"evm"`/`"solana"` (`runtime.rs` L87, L233, L423; tests L2905, L2937).
  Per-network tables need a table name that is not `'static`, such as the CAIP-2 string.
  CONFIRMED-CODE.
- **Two chains in one test are already possible.**
  - `Anvil::spawn_with_chain_id(base_port, chain_id)` exists and passes `--chain-id`
    ([`connector-settlement-evm/src/test_support.rs`](../../crates/connector-settlement-evm/src/test_support.rs)
    L152–171).
  - Each spawn takes its own port, so two anvils with distinct ids work now.
  - `require_anvil()` (L58) panics under `CI`. CONFIRMED-CODE.
  - The x402 contracts must be placed on each anvil, as the existing support already does per spawn.
    **Inference.**
- **The local stack** starts one anvil at 31337 (`infra/anvil/entrypoint.sh` L15). `seed.sh`'s
  addresses are nonce-deterministic (L42–82).
  - A multi-EVM topology needs a second `anvil` service with another `--chain-id`, which gets the
    same USDC address (§4).
  - Five topologies exist today, four on the CI gate (CLAUDE.md "What the containers ARE for";
    [`local/README.md`](../../local/README.md) L102–103).
  - CONFIRMED-CODE.
- **Committed fixtures that change.** CONFIRMED-CODE.
  - **13 TOML files** set `[settlement.evm]`:
    - `deploy/connector-rust/connector.production.toml`;
    - `infra/linode-relay/connector-rust.toml` and `infra/linode-store/connector-rust.toml`;
    - `local/{solo,anyone}/connector.toml`;
    - `local/{two-hop,mixed-chain,dealing,onion}/connector-{a,b}.toml`.
  - **138 occurrences** of the literal in Rust sources, the largest being `config.rs` 17, `error.rs`
    15, `runtime.rs` 15 and `connector-bin/tests/devnet_configs_load.rs` 15.
  - **Guard tests.** `devnet_configs_load.rs`, `local_topologies_load.rs`,
    `production_skeleton_is_inert.rs` and `documented_config_keys.rs`. The last checks
    `docs/protocol/configuration-spec.md`'s key table against the parser.
  - **The production skeleton.** It is "invalid on purpose" and must stay inert under the new shape.
  - **The real devnet configs live in the four node repos (ADR 0068).** Each must change in its own
    reviewed bump.

## 10. Canonical deployments (next-round Q9)

**`x402BatchSettlement` at `0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003`**, with collectors `…0004`
and `…0005`, is placed by CREATE2 and "anyone can deploy" it to the canonical address on any EVM chain
([X402 `contracts/evm/README.md` L19, L28–34, L80–83](https://github.com/x402-foundation/x402/blob/db2c98ec9490e4e4286c862de845e667b037f219/contracts/evm/README.md#L28-L34)).

**How each row was confirmed:**

- **README (CONFIRMED-DOCS):** the README's batch-settlement table (L49–64).
- **Code (CONFIRMED-CODE):** a live `eth_getCode` returned 22,352 hex characters, identical on every
  chain; `eth_chainId` was read in the same call.
- **USDC:** CIRCLE's mainnet table (L42–77) or testnet table (L102–134).
- **`name()`:** read live. `version()` is `"2"` everywhere read.

| Network             | Chain id | In README | Code present (live) | Circle USDC                                  | USDC `name()` |
| ------------------- | -------: | :-------: | :-----------------: | -------------------------------------------- | ------------- |
| Ethereum            |        1 |    no     |         yes         | `0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48` | `USD Coin`    |
| Optimism            |       10 |    yes    |         yes         | `0x0b2C639c533813f4Aa9D7837CAf62653d097Ff85` | `USD Coin`    |
| Unichain            |      130 |    yes    |         yes         | `0x078D782b760474a361dDA0AF3839290b0EF57AD6` | `USDC`        |
| Polygon PoS         |      137 |    yes    |         yes         | `0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359` | `USD Coin`    |
| Monad               |      143 |    yes    |         yes         | `0x754704Bc059F8C67012fEd69BC8A327a5aafb603` | `USDC`        |
| World Chain         |      480 |    yes    |         yes         | `0x79A02482A880bCe3F13E09da970dC34dB4cD24D1` | `USDC`        |
| Base                |     8453 |    yes    |         yes         | `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` | `USD Coin`    |
| Arbitrum One        |    42161 |    yes    |         yes         | `0xaf88d065e77c8cC2239327C5EDb3A432268e5831` | `USD Coin`    |
| Celo                |    42220 |    yes    |         yes         | `0xcebA9300f2b948710d2653dD7B07f33A8B32118C` | `USDC`        |
| Avalanche C-Chain   |    43114 |    yes    |         yes         | `0xB97EF9Ef8734C71904D8002F8b6Bc66Dd9c48a6E` | `USD Coin`    |
| Linea               |    59144 |    yes    |         yes         | `0x176211869cA2b568f2A7D4EE941E073a821EE1ff` | `USDC`        |
| World Chain Sepolia |     4801 |    yes    |         yes         | `0x66145f38cBAC35Ca6F1Dfb4914dF98F1614aeA88` | not read      |
| Monad Testnet       |    10143 |    yes    |         yes         | `0x534b2f3A21130d7a60830c2Df862319e593943A3` | not read      |
| Avalanche Fuji      |    43113 |    no     |         yes         | `0x5425890298aed601595a70AB815c96711a31Bc65` | not read      |
| Polygon Amoy        |    80002 |    no     |         yes         | `0x41E94Eb019C0762f9Bfcf9Fb1E58725BfB0e7582` | not read      |
| Base Sepolia        |    84532 |    no     |         yes         | `0x036CbD53842c5426634e7929541eC2318f3dCF7e` | `USDC`        |
| Arbitrum Sepolia    |   421614 |    no     |         yes         | `0x75faf114eafb1BDbe2F0316DF893fd58CE46AA4d` | `USD Coin`    |
| Ethereum Sepolia    | 11155111 |    no     |         yes         | `0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238` | not read      |

**Not deployed** (empty `eth_getCode`, CONFIRMED-CODE): OP Sepolia (11155420), Linea Sepolia (59141),
Unichain Sepolia (1301) and Celo Sepolia (11142220). Circle lists USDC on each.

**Where USDC is not native, or not what the devnet uses.**

- **The devnet.** It settles on Base Sepolia in **this project's own FiatToken**,
  `0x0C996d7c…C0E1` ([`infra/linode-relay/connector-rust.toml`](../../infra/linode-relay/connector-rust.toml)
  L136; #1337). That is not Circle's `0x036C…`, because the faucet must mint it (CLAUDE.md "Where
  money comes from"). A multi-EVM devnet needs the same arrangement on each added testnet. CONFIRMED-CODE.
- **Locally**, USDC is FiatToken at `0xe7f1…0512` on every seeded anvil (§4).
- **Bridged USDC.** Several of these chains also carry an older bridged USDC (`USDC.e`) at another
  address. CIRCLE lists only the native one, and that list is the source here. Which bridged tokens
  exist, and where, was **not** read: UNCONFIRMED.
- **Arc.** Its USDC is `0x3600…0000` on mainnet and testnet. Arc has x402's `exact`/`upto` proxies
  but is **not** in the batch-settlement table, so it is out of scope until x402 deploys there.
  CONFIRMED-DOCS.

## 11. What makes (a) harder than it looks

- **`SettlementChain::Evm` is a singleton key almost everywhere.** CONFIRMED-CODE (counted, inline
  test modules included).
  - It appears on 86 lines in 14 non-test-directory files, and `"evm"` on 117 lines in 33.
  - `runtime.rs` holds one `Option<Arc<EvmBatchSettlementBackend>>` (L882–884) and one transport slot
    (L1421–1424).
  - These are keyed by `SettlementChain` and need to become keyed by network: `restore_outbound_channels`'
    payer list (L1182–1215), `batch_channels_for_operator`'s receivers, `ClientChannelAssets`
    (`client_channel_asset.rs` L55–95), `x402_networks` and `RateSources`.
- **`ChannelPresentation::chain()` and `OutboundChannelRecord`'s `chain()` return `&'static str`
  `"evm"`** (`port.rs` L170–175, L333–338). That is a string, not a network.
- **The operator API's `chain` field** (`POST /channels`, `POST /peers`) and the app's
  `X-TOON-Chain` header both spell `evm`. Both are contracts with outside readers.
- **A cross-network forward on a dealing node needs a hand-tended static rate**, because quote paths
  cannot leave the numeraire's network (§8).
- **The `chain_id` name trap** on the channel rows (§5).
- **Mixed fleets during rollout.** An old node peering with a new one sees an extra `accepts[]`
  entry per added network.
  - `shared_settlement_of` on the **old** node returns `AmbiguousChain` when a new peer publishes two
    `eip155` entries. A new multi-network node cannot be peered by an old node until the old node
    upgrades. **Inference** (`peering.rs` L557–565).
- **The circuit-sharing regression** under `rpc_via_socks_proxy` if the circuit stays per chain (§5).

## Unconfirmed, and why

- **Bridged-USDC addresses** per chain. The source chosen was Circle's native list only (§10).
- **USDC `name()` on five testnets** (World Chain Sepolia, Monad Testnet, Fuji, Amoy, Ethereum
  Sepolia). They were not read, and §7's per-table recommendation does not depend on them.
- **Whether `name()` equals each token's EIP-712 domain name.** It does for FiatToken by construction,
  but was not checked against `DOMAIN_SEPARATOR()` here. `check_published_deposit_terms` does that at
  boot (`runtime.rs` L452–490).
- **Whether TC, `rig` and `swap` can absorb `network` in one release.** TC's terms parsing is ready
  (§2); its claim builder, and the other two repos, were not read for this.

## What the owner must still decide

- **D1. Does the journal channel key really need the chain id?**
  - **For keeping `evm:0x…`:** §1 makes it globally unique. Keeping it avoids the migration and the
    `X-TOON-Payer` change (§4).
  - **For adding the chain id:** routing a key to its backend without a lookup, and one rule for
    every identifier.
- **D2. Does Solana follow?**
  - Should Solana claims gain `network` (`solana:<genesis prefix>`) in the same schema bump?
  - Should Solana AssetIds move to CAIP-19 `solana:<genesis>/token:<mint>` now or later?
  - Doing it now makes every identifier uniform; deferring keeps the change EVM-only.
- **D3. Legacy state: upgrade in place, or refuse at boot?** Recomputation (§4) can assign every
  legacy entry by proof. ADR 0075's precedent was refusal, but for a scheme with no successor.
- **D4. Must a peering's two channels share one network?** It is legal by the channel model (§8). If
  allowed, the peering's two halves have different AssetIds, which matters to a dealing node.
- **D5. What do the app-facing headers say?** Is `X-TOON-Chain` widened to the CAIP-2 network, and
  does that need an ADR 0040 amendment?
- **D6. Is the multi-EVM `local/` topology on the CI gate?** If yes, it costs a second anvil service
  in an already expensive job.
- **D7. Which networks does the fleet run?** The devnet must mint its USDC (§10), and every added
  testnet needs a FiatToken the faucet can mint.
