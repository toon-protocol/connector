# x402 facilitators for the devnet's batch-settlement leg

> **Research note, not a decision record.** It decides nothing.
> [ADR 0074](../adr/0074-a-client-may-pay-over-an-x402-batch-settlement-channel.md) (Proposed) is
> the record this note serves. Its prerequisite 2 asks whether a hosted facilitator will relay a
> `batch-settlement` deposit on Base Sepolia to a connector's `payTo`. Primary sources only:
> live read-only `GET`s, each operator's own docs, and source code at a pinned commit. No `/verify`
> or `/settle` request was sent, and nothing was signed. Written 2026-09-25. Every endpoint was
> read between 10:59 and 11:20 UTC that day.

The question: which hosted or public x402 facilitators could TOON's devnet use for
`batch-settlement` on Base Sepolia (`eip155:84532`), and does any of them do SVM batch-settlement on
Solana devnet? Or should the devnet run the facilitator the local sandbox already runs?

A facilitator has one job here. It relays a client's gasless deposit into `x402BatchSettlement`
(`0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003`) and pays the gas. ADR 0074 adds two requirements:
the facilitator must **not** be the channel's `receiverAuthorizer` (decision 5), and it must accept
a `payTo` that is the connector's own EVM settlement address (decision 2).

**Evidence tags.** CONFIRMED-LIVE means read from a live endpoint or chain on 2026-09-25.
CONFIRMED-DOCS means read in the operator's own documentation. CONFIRMED-CODE means read in source at
a pinned commit. UNVERIFIED means none of those.

**Sources pinned.**

- **x402**: [`x402-foundation/x402@0cb1a1f0`](https://github.com/x402-foundation/x402/tree/0cb1a1f0f4c2163357e255c824d319674e1db43f),
  the same pin ADR 0074 uses. Cited below as **X402** plus a path.
- **BatchRail**: [`BatchRail/core@738a875d`](https://github.com/BatchRail/core/tree/738a875d8db1006a47f48f6db15071d8cd26ea9c).
- **The sandbox facilitator**: toon-protocol/infra `sandbox/x402-facilitator/` at `7c15d36` (the merge of infra#27).

---

## Verdict

**Three hosted facilitators offer `batch-settlement` on Base Sepolia, and one of them can be used
as-is: x402.org.** Its `/supported` lists `batch-settlement` on `eip155:84532` **with no `extra`**,
so it offers no `receiverAuthorizer` and none needs declining (CONFIRMED-LIVE). It needs no account.
It charges nothing. It has no `payTo` allowlist. It runs the same `@x402/evm` 2.27.0 TypeScript
code the sandbox facilitator runs (CONFIRMED-CODE).

The other two are worse fits:

- **CDP.** Its docs list Base Sepolia. It needs a CDP API key, screens the `payTo` for KYT and OFAC,
  and bills after 1,000 on-chain transactions a month.
- **BatchRail.** A hosted demo with a single maintainer. It advertises its own `receiverAuthorizer`,
  which the connector would have to decline. The protocol lets it decline that.

**No facilitator offers SVM `batch-settlement` on Solana devnet.** PayAI offers it only on Solana
**mainnet**, flagged `experimental` and `apiKeyRequired`. Its design seats the facilitator as
`feePayer`, which on SVM makes it the channel's `payee`. ADR 0074 decision 5 rules that out anyway:
on Solana the connector sponsors its own channels, so it needs no facilitator.

**Recommendation: the devnet should run its own facilitator.** Take the sandbox image and point it
at Base Sepolia. Use x402.org once, to close ADR 0074's prerequisite 2, and afterwards only as an
interop check, never as a dependency. The reasons are in the last section.

---

## Comparison

"Batch on 84532" and "Batch on Solana devnet" come from each facilitator's live `/supported`
response, read on 2026-09-25.

| Facilitator                                                                                 | Batch on 84532                                 | Batch on Solana devnet                             | `receiverAuthorizer` in `extra`                      | Auth                      | `payTo` screening                   | Cost on testnet                             | Implementation                     | Fit for the devnet        |
| ------------------------------------------------------------------------------------------- | ---------------------------------------------- | -------------------------------------------------- | ---------------------------------------------------- | ------------------------- | ----------------------------------- | ------------------------------------------- | ---------------------------------- | ------------------------- |
| **x402.org**                                                                                | **Yes**                                        | No                                                 | **None**                                             | None                      | None in code                        | Free                                        | `@x402/evm` TS (workspace, 2.27.0) | **Usable**                |
| **CDP**                                                                                     | Docs say yes; `/supported` returns 401         | No (docs)                                          | The API example shows one; the server may decline it | CDP API key ID and secret | KYT and OFAC on payer and recipient | 1,000 on-chain tx a month free, then $0.001 | Unknown (closed)                   | Usable, at a cost         |
| **BatchRail**                                                                               | **Yes**                                        | No                                                 | `0x1439fe67…4077` (decline it)                       | None by default           | None in code                        | Free                                        | `@x402/evm` ^2.23.0 TS             | Demo only                 |
| **PayAI**                                                                                   | No (`exact` only on EVM)                       | No. Mainnet only, `experimental`, `apiKeyRequired` | n/a (the SVM `feePayer` becomes `payee`)             | Key for batch             | Unknown                             | Devnet batch rates listed at $0             | PayAI fork of `@x402/svm` 2.24.0   | No                        |
| **Dexter**                                                                                  | No. Batch on Base mainnet and 5 other mainnets | No                                                 | `0x88559c29…ecfB`                                    | None stated               | Unknown                             | "No facilitator fee"                        | Express (header)                   | No (mainnet only)         |
| **Solvador**                                                                                | No. Batch on 11 EVM mainnets                   | No                                                 | `0xC077C1A9…F8e7`                                    | Unknown                   | Unknown                             | Unknown                                     | Express (header)                   | No (mainnet only)         |
| OpenX402, Daydreams, Mogami, xpay, Heurist, Meridian, Celo, HPP, Polygon, NEAR (mikedotexe) | No                                             | No                                                 | —                                                    | —                         | —                                   | —                                           | —                                  | No batch at all           |
| thirdweb                                                                                    | Unknown: `/supported` returns 401              | Unknown                                            | —                                                    | Secret key or client id   | —                                   | —                                           | —                                  | Docs do not mention batch |
| Corbits                                                                                     | Unknown: host did not resolve                  | Unknown                                            | —                                                    | —                         | —                                   | —                                           | Faremeter                          | Unreachable               |
| Fireblocks                                                                                  | Docs list no batch mechanism                   | —                                                  | —                                                    | Merchant API key          | —                                   | —                                           | Open source, Apache-2.0            | No                        |
| **Own (sandbox image)**                                                                     | After the change below                         | n/a                                                | None (by construction)                               | Ours                      | None                                | Gas only                                    | `@x402/evm` 2.27.0 TS              | **Recommended**           |

---

## 1. x402.org (`https://x402.org/facilitator`)

**`/supported`: CONFIRMED-LIVE on 2026-09-25 at 10:59 UTC.** It returned HTTP 200. The response
headers `server: cloudflare` and `x-vercel-id: iad1::…` show a Vercel deployment behind Cloudflare.
`https://www.x402.org/facilitator/supported` returns the identical body. The raw response:

```json
{
  "kinds": [
    { "x402Version": 2, "scheme": "exact", "network": "eip155:84532" },
    {
      "x402Version": 2,
      "scheme": "upto",
      "network": "eip155:84532",
      "extra": { "facilitatorAddress": "0xd407e409E34E0b9afb99EcCeb609bDbcD5e7f1bf" }
    },
    { "x402Version": 2, "scheme": "batch-settlement", "network": "eip155:84532" },
    {
      "x402Version": 2,
      "scheme": "exact",
      "network": "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1",
      "extra": {
        "feePayer": "CKPKJWNdJEqa81x7CkZ14BVPiY6y16Sxs7owznqtWYp5",
        "features": { "smartWalletSupported": true }
      }
    },
    {
      "x402Version": 2,
      "scheme": "exact",
      "network": "algorand:SGO1GKSzyE7IEPItTxCByw9x8FmnrCDe",
      "extra": { "feePayer": "G7QWRIJODICBDG6JAVXNKHNTCKTBJZBXTSCGQLSMXSCIKEJ5SNFPEJSFQQ" }
    },
    {
      "x402Version": 2,
      "scheme": "exact",
      "network": "aptos:2",
      "extra": { "feePayer": "0x1be1a717b48c46c83a2a6a53205aff6123610961560b2b08968a344c4da24b1e" }
    },
    {
      "x402Version": 2,
      "scheme": "exact",
      "network": "stellar:testnet",
      "extra": { "areFeesSponsored": true }
    },
    {
      "x402Version": 2,
      "scheme": "exact",
      "network": "hedera:testnet",
      "extra": { "feePayer": "0.0.9185802" }
    },
    {
      "x402Version": 2,
      "scheme": "exact",
      "network": "xrpl:1",
      "extra": { "areFeesSponsored": false }
    },
    { "x402Version": 1, "scheme": "exact", "network": "base-sepolia" },
    {
      "x402Version": 1,
      "scheme": "exact",
      "network": "solana-devnet",
      "extra": { "feePayer": "CKPKJWNdJEqa81x7CkZ14BVPiY6y16Sxs7owznqtWYp5" }
    }
  ],
  "extensions": ["builder-code", "eip2612GasSponsoring", "erc20ApprovalGasSponsoring"],
  "signers": {
    "eip155:*": ["0xd407e409E34E0b9afb99EcCeb609bDbcD5e7f1bf"],
    "solana:*": ["CKPKJWNdJEqa81x7CkZ14BVPiY6y16Sxs7owznqtWYp5"],
    "algorand:*": ["G7QWRIJODICBDG6JAVXNKHNTCKTBJZBXTSCGQLSMXSCIKEJ5SNFPEJSFQQ"],
    "aptos:*": ["0x1be1a717b48c46c83a2a6a53205aff6123610961560b2b08968a344c4da24b1e"],
    "stellar:*": [
      "GC6CSXBV4C6RL3HEDTW57KXYXSSXKAWKGYDEOSATXM3XNKXSR2VRYN3K",
      "GC5OLUZ4WANPN6VT7YGTK2SRMZG762KOVKJXHWIO4K57UBASO2FMNRET"
    ],
    "hedera:*": ["0.0.9185802"],
    "xrpl:*": []
  }
}
```

1. **Batch on 84532: yes. Batch on Solana devnet: no.** Solana devnet appears only under `exact`
   (CONFIRMED-LIVE). The response is unchanged from what ADR 0074 recorded on 2026-09-24.
2. **`receiverAuthorizer`: none advertised** (CONFIRMED-LIVE). The source shows why. The site
   registers `new BatchSettlementEvmScheme(evmSigner)` with no `authorizerSigner`
   (X402 `typescript/site/app/facilitator/index.ts#L180`). Without one, the scheme advertises no
   `receiverAuthorizer`, "and servers supply their own signatures"
   (X402 `typescript/packages/mechanisms/evm/src/batch-settlement/facilitator/scheme.ts#L68-L74`)
   (CONFIRMED-CODE). So a server **must** bring its own authorizer, which is exactly what ADR 0074
   wants.
3. **Auth: none.** The x402 docs say it "requires no setup"
   ([`docs/core-concepts/network-and-token-support.mdx#L317-L320`](https://github.com/x402-foundation/x402/blob/0cb1a1f0f4c2163357e255c824d319674e1db43f/docs/core-concepts/network-and-token-support.mdx#L317-L320))
   (CONFIRMED-DOCS). `/supported` answered without credentials (CONFIRMED-LIVE).
4. **`payTo` restrictions: none in the code.** The facilitator checks
   `config.receiver == requirements.payTo` and `config.receiverAuthorizer == extra.receiverAuthorizer`
   (X402 `…/batch-settlement/facilitator/utils.ts#L166-L178`). It checks `withdrawDelay` against
   `extra.withdrawDelay` only when `extra.withdrawDelay` is present, and against the contract's
   15-minute to 30-day range (`utils.ts#L185-L190`) (CONFIRMED-CODE). `BatchSettlementEvmScheme` is
   registered without the `eip6492AllowedFactories` list, so a counterfactual smart-wallet payer is
   refused, while an EOA or already-deployed wallet is not (`index.ts#L180`). No screening appears
   anywhere in `typescript/site/app/facilitator/`.
5. **Rate limits and pricing.** The site's facilitator routes contain no rate limiter (CONFIRMED-CODE:
   there is no match for `ratelimit|429` in `typescript/site/app/facilitator/` or `lib/`). Any limit
   Vercel or Cloudflare applies is UNVERIFIED. It is free, with no pricing anywhere in the docs
   (CONFIRMED-DOCS).
6. **Implementation.** TypeScript `@x402/core` and `@x402/evm` as `workspace:*`
   (`typescript/site/package.json#L23-L34`), which are both `2.27.0` at the pin (CONFIRMED-CODE).
   That is the same published version the infra sandbox pins. Which commit is actually deployed on
   Vercel is UNVERIFIED.
7. **Operator and disclaimers.** It is run by the x402 Foundation, from the x402 repository's own
   `typescript/site`. The docs say it "is intended for development and testnet workflows"
   ([`docs/core-concepts/facilitator.md#L35`](https://github.com/x402-foundation/x402/blob/0cb1a1f0f4c2163357e255c824d319674e1db43f/docs/core-concepts/facilitator.md#L35))
   and "is not intended for mainnet routes"
   ([`docs/dev-tools/facilitators.md#L38`](https://github.com/x402-foundation/x402/blob/0cb1a1f0f4c2163357e255c824d319674e1db43f/docs/dev-tools/facilitators.md#L38))
   (CONFIRMED-DOCS). No SLA and no status page were found.
   - **An open bug on this exact path.** [x402#2471](https://github.com/x402-foundation/x402/issues/2471)
     is titled _"Hosted facilitator: claim → settle ordering produces `replacement transaction
underpriced` and `nothing_to_settle`"_. It reports that x402.org on Base Sepolia does not
     serialise same-sender nonces and reads stale state. It was opened 2026-05-26, is still OPEN,
     and has no maintainer reply (CONFIRMED-LIVE via `gh`).
   - **Why that matters for deposits.** Under ADR 0074 the connector claims and settles for itself,
     so the claim-then-settle race does not touch it. But the **nonce** half applies to every
     transaction the one signer sends, including other tenants' deposits.
8. **Settlement and gas.** The facilitator's one EVM signer, `0xd407e409…f1bf`, pays gas. On
   2026-09-25 it held 0.829 ETH on Base Sepolia at nonce 2,286,974 (CONFIRMED-LIVE,
   `eth_getBalance` and `eth_getTransactionCount` against `https://sepolia.base.org`). Nothing is
   charged.
   - **What the signer has actually done on the batch contract.** Blockscout lists the 750 most
     recent transactions to `0x4020…0003`, from 2026-09-22 00:49 to 2026-09-24 21:55 UTC. In that
     sample this signer sent **3 `claimWithSignature` and 1 `settle`, and no `deposit`**. The
     sample's 393 deposits came from other senders (CONFIRMED-LIVE,
     `base-sepolia.blockscout.com/api/v2/addresses/0x4020…0003/transactions`).
   - **So the hosted deposit path is unobserved.** It is not shown to be broken, but it is not shown
     to work either. The deployed code is the same code the infra sandbox has run.

---

## 2. Coinbase CDP (`https://api.cdp.coinbase.com/platform/v2/x402`)

1. **Supported networks.**
   - **Live check: HTTP 401.** `GET /supported` returned body `Unauthorized` (CONFIRMED-LIVE). The
     OpenAPI reference marks the endpoint `apiKeyAuth`
     ([reference](https://docs.cdp.coinbase.com/api-reference/v2/rest-api/x402-facilitator/get-supported-payment-schemes-and-networks))
     (CONFIRMED-DOCS).
   - **The docs' network matrix** lists `batch-settlement` on Base, **Base Sepolia**, Polygon,
     Arbitrum, World and World Sepolia. Solana and Solana Devnet get `exact` and `upto` only
     ([seller/facilitator](https://docs.cdp.coinbase.com/x402/seller/facilitator), which
     `/x402/network-support` redirects to) (CONFIRMED-DOCS). The FAQ says "`batch-settlement`
     remains EVM-only" ([FAQ](https://docs.cdp.coinbase.com/x402/support/faq)).
2. **`receiverAuthorizer`: advertised in the docs' example.** The `/supported` example response for
   `eip155:8453` carries `extra.receiverAuthorizer` (CONFIRMED-DOCS, the same OpenAPI page). Whether
   the Base Sepolia entry carries one is UNVERIFIED, since the endpoint needs auth.
   - **A server can decline it.** The spec: "The server may delegate to this address as its
     channel's `receiverAuthorizer`, or supply its own"
     (X402 `specs/schemes/batch-settlement/scheme_batch_settlement_evm.md#L468`).
   - **The reference server code implements that choice.** A configured `receiverAuthorizerSigner`
     wins over the facilitator's advertised one (X402
     `…/batch-settlement/server/scheme.ts#L308-L312`). The startup check is skipped when the server
     brings its own (`#L346`) (CONFIRMED-CODE).
   - **The facilitator then enforces the server's choice.** It compares the channel against the
     requirements' `extra.receiverAuthorizer`, not against its own (`facilitator/utils.ts#L171-L178`).
   - **The one caveat.** CDP runs closed code, so whether it applies the same equality or insists on
     its own address is UNVERIFIED.
3. **Auth.** "The CDP Facilitator authenticates with your CDP API key ID and secret" (CONFIRMED-DOCS,
   seller/facilitator). The key comes from a CDP account. The FAQ points to the CDP Portal for Base
   Sepolia funds (CONFIRMED-DOCS). The exact request-signing scheme (a per-request JWT) was not read
   and is UNVERIFIED. It matters because the connector is Rust, and the reference helper
   (`createCdpFacilitatorClient`) is TypeScript.
4. **`payTo` screening: yes.** "Every payment is screened against OFAC sanctions lists and Know Your
   Transaction (KYT) risk signals before it settles. … Screening runs at both verification and
   settlement, and checks the payer and the recipient." A declined payment fails with
   `kyt_risk_detected` (FAQ, CONFIRMED-DOCS). There is no registration of receiving addresses:
   "Payments settle directly to the `payTo` address configured on the route" (FAQ).
5. **Rate limits and pricing.**
   - **Pricing:** "The first 1,000 onchain Facilitator transactions each month are free, then each
     additional onchain transaction costs $0.001". For `batch-settlement`, "deposits, refunds, and
     withdrawals, each count as one onchain transaction" (seller/facilitator, CONFIRMED-DOCS).
   - **What is billed:** only a 2xx settle; verification is free (FAQ). Whether testnet settlements
     count toward the 1,000 is **not stated** (UNVERIFIED).
   - **Rate limits:** "standard CDP API rate limiting … rejected with `429`", and no figure is given
     (FAQ).
6. **Implementation.** Closed and UNVERIFIED. The x402 site's EIP-6492 factory list says it was
   "Ported from the CDP Facilitator's allowlist" (X402 `typescript/site/app/facilitator/index.ts#L124`),
   which suggests lineage but proves nothing. A related CDP-SDK fact: "`createX402Server` does not
   register `batch-settlement` today. Server support was withdrawn because the scheme registered
   without running the channel settle lifecycle" (FAQ, CONFIRMED-DOCS). That concerns CDP's
   **server** SDK, not the facilitator, and TOON would not use it.
7. **Operator and SLA.** Coinbase. The docs cite a "99.9% availability" target on the CDP SLO page
   (FAQ, CONFIRMED-DOCS). The docs also sell it as "Move from testnet to mainnet with one provider"
   (seller/facilitator).
8. **Settlement and gas.** "The facilitator submits the settlement transaction and pays the gas,
   which is what the per-transaction price covers" (FAQ, CONFIRMED-DOCS).

---

## 3. BatchRail (`https://facilitator.batchrail.io`)

It is not on x402's facilitator list. It turned up in a web search and is included because it is the
only other live host of batch-settlement on 84532.

**`/supported`: CONFIRMED-LIVE on 2026-09-25, HTTP 200.** The headers were `server: railway-hikari`
and `x-powered-by: Express`. The raw response:

```json
{
  "kinds": [
    {
      "x402Version": 2,
      "scheme": "batch-settlement",
      "network": "eip155:84532",
      "extra": { "receiverAuthorizer": "0x1439fe67f5d5023183d3A3835fdC499dd5FE4077" }
    },
    { "x402Version": 2, "scheme": "exact", "network": "eip155:84532" },
    { "x402Version": 1, "scheme": "exact", "network": "base-sepolia" }
  ],
  "extensions": [],
  "signers": { "eip155:84532": ["0x1439fe67f5d5023183d3A3835fdC499dd5FE4077"] }
}
```

1. **Batch on 84532: yes. On Solana: no** (CONFIRMED-LIVE). Its code allows only `eip155:84532` and
   `base-sepolia` (`packages/facilitator/src/index.ts#L35`, CONFIRMED-CODE).
2. **`receiverAuthorizer`: advertised, and it is the relaying signer itself.** It is built as
   `new BatchSettlementEvmScheme(facilitatorSigner, authorizerAccount)` (`index.ts#L66-L68`). A
   server can decline it by supplying its own authorizer, as in §2.2, because this is the stock TS
   facilitator (CONFIRMED-CODE). Declining is mandatory for TOON.
   - **An extra risk if a server did not decline.** The spec requires a facilitator that advertises
     a `receiverAuthorizer` to authenticate refund requests (X402
     `scheme_batch_settlement_evm.md#L674`). BatchRail's `API_KEY` is optional (`index.ts#L11`).
3. **Auth: none on the public demo.** "No API key required for this public demo endpoint"
   (README L21, CONFIRMED-DOCS).
4. **`payTo` restrictions: none beyond the stock checks** (CONFIRMED-CODE).
5. **Rate limits: an in-memory per-IP bucket**, `RATE_LIMIT_PER_MIN`, default 120
   (`index.ts#L30-L32`, `#L216-L236`) (CONFIRMED-CODE). It is free.
6. **Implementation.** TypeScript on `@x402/core` and `@x402/evm` `^2.23.0`
   (`packages/facilitator/package.json`) (CONFIRMED-CODE).
7. **Operator and disclaimers.** BatchRail, a repository with 0 stars, last pushed 2026-09-24. It
   describes itself as an "Open-source local demo" (README L5) (CONFIRMED-LIVE via `gh`). No SLA.
8. **Settlement and gas.** `0x1439fe67…` pays. The same address appears on chain sending `deposit`,
   `claimWithSignature`, `settle` and `refundWithSignature` to `0x4020…0003` (CONFIRMED-LIVE,
   Blockscout sample above). Nothing is charged.

---

## 4. PayAI (`https://facilitator.payai.network`)

**`/supported`: CONFIRMED-LIVE on 2026-09-25, HTTP 200.** A `ratelimit-policy: "payments-read";q=200;w=1`
header was present. The response lists **no EVM `batch-settlement` at all**: EVM has `exact` only,
including `eip155:84532`. Its one batch entry:

```json
{
  "x402Version": 2,
  "scheme": "batch-settlement",
  "network": "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
  "extra": {
    "feePayer": "CjNFTjvBhbJJd2B5ePPMHRLx1ELZpa8dwQgGL727eKww",
    "experimental": true,
    "apiKeyRequired": true,
    "batchPolicy": {
      "asset": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
      "minInitialDeposit": "10000",
      "maxInitialDeposit": "100000000",
      "maxWithdrawDelay": 86400,
      "maxOpenChannels": 1000,
      "maxOpenChannelsPerAccount": 10,
      "depositAttemptsPerMinute": 5,
      "idleCloseSeconds": 259200
    }
  }
}
```

1. **Solana mainnet only.** The response's own `pricing.rates` table lists `batch-settlement` rates
   for Solana **devnet** (`solana:EtWTR…`) at `"usd":"0"`, but no devnet `batch-settlement` kind is
   advertised (CONFIRMED-LIVE). So devnet batch is priced but not offered.
2. **It is structurally unusable for TOON.** On SVM, x402's `extra.feePayer` becomes the channel's
   `rent_payer` and zero-share `payee` (X402 `scheme_batch_settlement_svm.md#L36-L49`, `#L109-L110`).
   ADR 0074 decision 5 rejects a third-party `payee`: after `request_close`, only the payee can land
   a voucher. PayAI's own release notes confirm the shape: the merchant must "call
   `BatchChannelManager.sealClosingChannel(channelId)`", and "PayAI does not store unclaimed vouchers
   for offline merchants". The preview also runs a 72-hour idle cleanup
   ([release `payai-batch-preview-20260922-seal`](https://github.com/PayAINetwork/x402-batch-preview/releases/tag/payai-batch-preview-20260922-seal))
   (CONFIRMED-DOCS).
3. **Auth.** x402's list says "No API keys required"
   ([`docs/dev-tools/facilitators.md#L33`](https://github.com/x402-foundation/x402/blob/0cb1a1f0f4c2163357e255c824d319674e1db43f/docs/dev-tools/facilitators.md#L33)).
   The batch entry itself says `apiKeyRequired: true` (CONFIRMED-LIVE).
4. **Implementation.** "`@x402/svm` 2.24.0 built from PayAI source commit `99daf2938`", porting the
   upstream seal flow from the open PR (release notes, CONFIRMED-DOCS). These are preview tarballs,
   not an npm release.

---

## 5. Mainnet-only batch facilitators: Dexter and Solvador

Neither lists `batch-settlement` on any testnet, so neither can serve the devnet. They are recorded
because they are the hosted options should mainnet ever need one.

- **Dexter** (`https://x402.dexter.cash`). CONFIRMED-LIVE on 2026-09-25, HTTP 200, `x-powered-by:
Express`.
  - **Batch networks:** `eip155:8453`, `137`, `42161`, `480`, `143` and `4663`, each with
    `extra.receiverAuthorizer: "0x88559c293Aa9A27707e66CE69F0b40eb8E9aecfB"`. On `eip155:84532` it
    offers only `exact` and `upto`.
  - **Solana:** `exact` on mainnet and devnet, plus a bespoke `tab` scheme on mainnet. No batch.
  - **Docs:** "Free public x402 facilitator … with no fees and no account required"
    ([`docs/dev-tools/facilitators.md#L26`](https://github.com/x402-foundation/x402/blob/0cb1a1f0f4c2163357e255c824d319674e1db43f/docs/dev-tools/facilitators.md#L26),
    CONFIRMED-DOCS). Its own page says "Dexter charges no facilitator fee"
    ([dexter.cash/facilitator](https://dexter.cash/facilitator)). It does not document receiver
    authorizers, testnets or rate limits.
- **Solvador** (`https://api.solvador.com`). CONFIRMED-LIVE on 2026-09-25, HTTP 200.
  - **Batch networks:** 11 EVM mainnets (`8453`, `42161`, `10`, `137`, `43114`, `42220`, `59144`,
    `130`, `480`, `143`, `4663`), each with `extra.receiverAuthorizer: "0xC077C1A915A61021b16d1581067C828b7C76F8e7"`.
  - **Testnets and Solana:** no testnet kind of any scheme, and `exact` only on Solana.

---

## 6. Everything else on x402's list, and the community names

x402's own list is [`docs/dev-tools/facilitators.md`](https://github.com/x402-foundation/x402/blob/0cb1a1f0f4c2163357e255c824d319674e1db43f/docs/dev-tools/facilitators.md)
at the pin. `https://www.x402.org/ecosystem` returned **404** on 2026-09-25, so that list is the
canonical one. Every row below was read live on 2026-09-25 unless it says otherwise:

| Facilitator                            | Endpoint read                                 | Result                                                                                                                                                                                   |
| -------------------------------------- | --------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| OpenX402                               | `facilitator.openx402.ai/supported`           | 200. `exact` only, including on 84532. No batch.                                                                                                                                         |
| Daydreams                              | `facilitator.daydreams.systems/supported`     | 200. `exact` and `upto`, including on 84532, and `exact` on Solana devnet. No batch.                                                                                                     |
| Mogami                                 | `facilitator.mogami.tech/supported`           | 200. v1 `exact` on `base-sepolia` and `base` only.                                                                                                                                       |
| xpay                                   | `facilitator.xpay.sh/supported`               | 200. `exact` on 8453 and 84532. No batch.                                                                                                                                                |
| Heurist                                | `facilitator.heurist.xyz/supported`           | 200. `exact` on Base, XLayer and Stable mainnets. No testnet, no batch.                                                                                                                  |
| Meridian                               | `api.mrdn.finance/v1/supported`               | 200. v1 `exact` and `upto` only, including `base-sepolia` and `solana-devnet`. No batch.                                                                                                 |
| Celo                                   | `api.x402.celo.org/supported`                 | 200. `exact` on Celo mainnet only.                                                                                                                                                       |
| HPP                                    | `facilitator.hpp.io/supported`                | 200. `exact` and `upto` on `eip155:190415` only.                                                                                                                                         |
| Polygon                                | `x402.polygon.technology/supported`           | 200. `exact` on Polygon mainnet only.                                                                                                                                                    |
| NEAR (mikedotexe)                      | `x402.mikedotexe.com/supported`               | 200. `exact` on `near:mainnet` only.                                                                                                                                                     |
| thirdweb                               | `api.thirdweb.com/v1/payments/x402/supported` | 401, `"x-secret-key or x-client-id header required"`. Its [docs](https://portal.thirdweb.com/x402/facilitator) describe single settlement from your own server wallet, with no batch.    |
| Corbits                                | `facilitator.corbits.dev/supported`           | The host did not resolve (curl error 6), and the listed docs page returned 404. UNVERIFIED.                                                                                              |
| Fireblocks                             | Docs only                                     | The [overview](https://developers.fireblocks.com/docs/x402-facilitator-overview) lists `eip-3009`, `permit2`, `upto-permit2` and `erc7710`, and no batch. It is testnet-only by default. |
| Built on Stellar, FTP Canton, T54 XRPL | Not probed                                    | Non-EVM and non-Solana, so out of scope.                                                                                                                                                 |

---

## 7. Our own: the sandbox image against Base Sepolia

toon-protocol/infra `sandbox/x402-facilitator/` (at `7c15d36` (the merge of infra#27)) is an Express app over the published
`@x402/core` and `@x402/evm` **2.27.0**. It registers `BatchSettlementEvmScheme` with no authorizer,
so it offers no `receiverAuthorizer` (`index.mjs#L59-L60`, and the header at `#L12`). That is the
same code, at the same version, that x402.org runs (§1.6), so the two will behave identically on the
zero-voucher question ADR 0074's prerequisite 1 settled.

**It is not yet parameterised for Base Sepolia.**

- **What is hard-coded:** `NETWORK = "eip155:31337"` and `defineChain({ id: 31337 })`
  (`index.mjs#L30`, `#L38-L42`). Only `EVM_RPC_URL`, `PORT` and `FACILITATOR_EVM_PRIVATE_KEY` come
  from the environment.
- **The key default:** it falls back to an anvil-mnemonic key (`#L33-L36`).
- **What pointing it at 84532 takes:**
  - make the network and chain id configurable;
  - drop the key default, so a missing key fails loudly;
  - give it a Base Sepolia ETH-funded key.

That is a small change in infra, not here. Its `/health` already refuses to report healthy unless
`x402BatchSettlement` has code on the chain (`#L92-L98`). The contract is on Base Sepolia at its
production address (ADR 0074, Sources).

---

## Recommendation for the devnet

**Run our own facilitator: the sandbox image, parameterised for `eip155:84532`.** Do not depend on a
hosted one. Use x402.org once, to close ADR 0074's prerequisite 2, and keep it only as an interop
probe. Here is why.

1. **Hosting it ourselves gains nothing in code and removes a third party.** x402.org is the only
   hosted option that fits cleanly, and it runs exactly our code (`@x402/evm` 2.27.0 TS). Using it
   buys no behaviour we lack. It adds an operator that disclaims production use, has no SLA or
   status page, deploys an unknown commit, and has an open, unanswered bug (x402#2471) on
   same-signer nonce handling for batch-settlement on this very network. One signer is shared by
   every tenant.
2. **The hosted deposit path is unobserved.** In the 750 most recent transactions to the batch
   contract, x402.org's signer relayed no deposit. The sandbox has run this path under
   `make smoke-x402`. A devnet should rest on the path we have seen work.
3. **CDP is the wrong trade for a devnet.**
   - **Auth:** it needs a CDP account and a per-request API-key auth the Rust connector would have
     to implement.
   - **Screening:** it screens the connector's own `payTo` for KYT and OFAC, which is a
     failure mode the devnet has no use for.
   - **Documentation gaps:** whether Base Sepolia settlements consume the 1,000 free transactions is
     undocumented, and its `/supported` cannot be read without a key.
   - **Where it earns its place:** it is the credible hosted option for a **mainnet** operator, and
     that is where it should be evaluated. Dexter and Solvador are the others there. All three
     advertise a `receiverAuthorizer` the connector must decline.
4. **BatchRail and PayAI are out.** BatchRail is a single-maintainer demo whose advertised authorizer
   is its own relaying key. PayAI offers no EVM batch and only mainnet, experimental SVM batch, in a
   shape ADR 0074 forbids.
5. **Solana needs no facilitator at all.** Under ADR 0074 decision 5 the connector is its own SVM
   sponsor, and no facilitator offers SVM batch on devnet anyway. Upstream SVM support is still an
   open PR ([x402#3164](https://github.com/x402-foundation/x402/pull/3164): OPEN, updated
   2026-09-24), and the SVM spec is marked "Status: **draft**" (X402
   `scheme_batch_settlement_svm.md#L3`). At the pin, neither TS nor Go has an SVM batch-settlement
   mechanism.
6. **The ADR's claim survives either way.** "On EVM a stock x402 facilitator relays the deposit"
   stays true whether that facilitator is x402.org or our copy of the same package. The connector's
   side does not change: it names its own `receiverAuthorizer` and puts its own address in `payTo`.
   A later switch to a hosted facilitator is a URL change.

**Where it runs, and whose gas key it holds, is an operations question this note does not answer.**
The faucet box is the obvious candidate, since it already holds devnet keys. Whether the faucet
holds Base Sepolia **ETH**, rather than only minting USDC, was not checked.

---

## What is unverified

- **Whether x402.org actually relays a `batch-settlement` deposit to an arbitrary `payTo`.** Its code
  says yes. No live deposit was attempted, since this note sends no `/settle`. That is exactly ADR
  0074's open prerequisite 2, and it stays open.
- **Which commit x402.org has deployed.** The site builds from workspace packages. The Vercel deploy
  revision is not public.
- **Whether CDP's Base Sepolia batch entry advertises a `receiverAuthorizer`, and whether CDP
  accepts a server-supplied one.** Its `/supported` needs auth, and its code is closed. The spec and
  the reference code both allow declining; CDP's own enforcement is unknown.
- **Whether CDP bills testnet settlements** against the 1,000 a month, and the exact form of its
  API-key request signing.
- **Any platform-level rate limit on x402.org** (Vercel or Cloudflare).
- **Who sends the other 393 sampled deposits** (`0x22c4…EeEb`, `0xCACe…41A8`, `0x5f9c…8F52` and
  others). They may be CDP's signers or other facilitators. They were not attributed.
- **Corbits**, whose hosted endpoint did not resolve, and **thirdweb**, whose `/supported` needs a
  key. Either might offer batch-settlement behind that.
- **Dexter's and Solvador's** `payTo` screening, rate limits, and whether they honour a
  server-supplied `receiverAuthorizer`. This is moot for the devnet, since neither offers a testnet
  batch kind.
- **Whether the faucet box holds Base Sepolia ETH** to fund our own facilitator's gas key.

## Update, 2026-09-25: x402.org's deposit path, run live

The "What is unverified" section above lists whether x402.org's hosted deposit path works as open.
It has now been run once, and ADR 0074's prerequisite 2 records the details.

**It works for an arbitrary `payTo` and our own token.** A Permit2 deposit of the devnet's mock USDC
(`0x49beE1Bc…a9Ce`) was relayed by x402.org's signer `0xd407e409…f1bf`. The receiver and
`receiverAuthorizer` were an address the facilitator had never seen. The result is transaction
`0x54e792b8…d7a5`, which left 1 mock USDC in channel `0x25712fc6…a8ec`.

**`erc20ApprovalGasSponsoring` does not work there.** x402.org advertises the extension, but the
first attempt broadcast the payer's signed `approve` without first funding the payer, and failed
with insufficient funds. The operator is supposed to supply that funding step around the package,
as x402's own e2e facilitator does with its `sendTransactions` wrapper. x402.org apparently runs
without it.

This strengthens the recommendation above. The devnet's token has no ERC-3009, so a gasless devnet
deposit needs a facilitator that performs the funding step, and ours can.
