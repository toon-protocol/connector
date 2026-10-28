# How tokens work on Mina, and whether it has a stablecoin

> **Research note, not a decision record.** It decides nothing.
> [ADR 0080](../adr/0080-a-client-may-pay-from-a-zk-note.md) (_A client may pay from a zk note_) is
> the record this note serves, and the question comes from triage on #1513. That triage proposes the
> asset string `mina:mainnet/slip44:12586` (and `mina:devnet/slip44:12586`) with 9 decimals,
> because #1511 puts "tokens on Mina, including USDC" out of scope. This note checks that string
> against the registries that own each part of it, describes Mina's token model, and records which
> stablecoins exist on Mina today. Primary sources only. Written 2026-10-27, and every source was
> read that day.

**Evidence tags.** CONFIRMED-CODE means read in source at a pinned commit. CONFIRMED-REGISTRY means
read in the registry file that owns the fact, at a pinned commit. CONFIRMED-DOCS means read in the
maintainer's or issuer's own documents. UNCONFIRMED means none of those, and says why.

**Sources pinned.**

- **Mina daemon**: [`MinaProtocol/mina@96f857bb`](https://github.com/MinaProtocol/mina/tree/96f857bbf4f6ff2e144ea26ce10e247487a07970)
  (the `compatible` branch, 2026-10-27). Cited below as **MINA** plus a path.
- **o1js**: [`o1-labs/o1js@a428f9b9`](https://github.com/o1-labs/o1js/tree/a428f9b97817177ef4b5868cf148c0ac67a1048d)
  (`main`, 2026-10-27; the latest release is `v3.1.0`). Cited as **O1JS** plus a path.
- **Mina docs**: [`MinaProtocol/docs2@5e910fbc`](https://github.com/MinaProtocol/docs2/tree/5e910fbc293982f63d597a0821b7c8bffb2aa63c),
  the source of docs.minaprotocol.com.
- **MIPs**: [`MinaProtocol/MIPs@5aa77c89`](https://github.com/MinaProtocol/MIPs/tree/5aa77c8911e7af39ff9c594175226b171ed8166d).
- **SLIP-0044**: [`satoshilabs/slips@570ed55b`](https://github.com/satoshilabs/slips/blob/570ed55b7fde158f1116be34fc2faa35dada5912/slip-0044.md)
  (the last commit to touch `slip-0044.md`, 2026-09-01).
- **CAIPs**: [`ChainAgnostic/CAIPs@f46c0fe0`](https://github.com/ChainAgnostic/CAIPs/tree/f46c0fe052eb80875dd94bbb39af16bb96ad6c1a).
- **Namespaces**: [`ChainAgnostic/namespaces@463bae56`](https://github.com/ChainAgnostic/namespaces/tree/463bae5625aaa8c184ccc74f1db457da8a43e72b).
  This is the same commit the spike pinned, and it is still `HEAD`, so nothing has changed since.

---

## Verdict

**The proposed string holds up, with one caveat about its form.** `mina:mainnet/slip44:12586` is
built entirely from registered parts: the chain part is the `mina` namespace's own CAIP-2
reference, `slip44` is the one cross-namespace asset namespace CAIP-19 defines, and **12586 is MINA's
coin type in the SLIP-0044 registry file itself**. **9 decimals is right**: the daemon defines a
MINA as 10^9 nanomina. The string is TOON's own composition, not a registry entry, because the `mina`
namespace has no CAIP-19 profile and no open PR proposing one. The caveat is that it is
a **CAIP-19** string, while the connector's own asset identity (`AssetId`, ADR 0071) is a
`<chain>:<token>` string that is not CAIP-19 and carries no decimals. #1522 has to decide which form
the zk-note table uses. See §5.

**There is no live stablecoin on Mina mainnet.** Circle doesn't list Mina among its USDC chains, and
Tether doesn't list it among its USDT protocols. The one Mina-native stablecoin project (zkUSD, later
called Fizk) last reported running on devnet only, with mainnet launch waiting on legal advice and an
audit. Nori, the Ethereum-to-Mina bridge, has only a staging deployment on Mina devnet, and its own
runbook lists open items that must be resolved before mainnet. It bridges ETH, not a stablecoin.
#1511's "tokens on Mina, **including USDC**" names an asset that doesn't exist on Mina.

**Native-only does not paint the connector into a corner.** On Mina a token is a separate ledger
account under a different token ID. A vault holding a custom token would therefore be a different
zkApp with a different verification key, published under a different asset string. The connector
never touches the vault's token. It needs only a configured asset string and a decimals figure that
isn't hardcoded to 9. The work a token would require is in the vault's proof programs, outside this
repository (§5.3).

| #   | Question                        | Answer                                                                                                                                                 | Tag                      |
| --- | ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------ |
| 1   | MINA's smallest unit / decimals | **nanomina, 9 decimals** (`precision = 9`, `unit_to_nano = 1_000_000_000`); amounts are `uint64`                                                       | CONFIRMED-CODE           |
| 1   | SLIP-44 coin type               | **12586**, symbol `MINA`; Mina's own docs derive keys at `m/44'/12586'/…`                                                                              | CONFIRMED-REGISTRY, DOCS |
| 2   | Token identity                  | `tokenId = Poseidon("MinaDeriveTokenId", ownerPublicKey ‖ parentTokenId)`; MINA is token ID `1` (`wSHV2S4qX9jFsLjQo8r1BsMLH2ZRKsZx6EJd1sbozGPieEC4Jf`) | CONFIRMED-CODE           |
| 2   | Decimals of a custom token      | **Not a protocol field.** The fungible-token standard keeps `decimals` in its zkApp state                                                              | CONFIRMED-CODE           |
| 3   | `mina` in ChainAgnostic         | CAIP-2 only (Draft, Mina Foundation): `mina:mainnet`, `mina:devnet`, human-readable names, no genesis hash. **No CAIP-10, no CAIP-19, no open PR**     | CONFIRMED-REGISTRY       |
| 3   | Does `slip44` fit native MINA   | Yes. CAIP-20 delegates the reference to SLIP-44's index; the Stellar profile uses the same form for its native asset on mainnet and testnet            | CONFIRMED-REGISTRY       |
| 4   | Stablecoins on Mina             | **None live on mainnet.** zkUSD/Fizk: devnet only. No Circle USDC, no Tether USDT. Nori bridge: Mina devnet staging, ETH only                          | CONFIRMED-DOCS           |
| 5   | Is the proposed string right    | Yes, as a CAIP-19 string. Whether the zk-note table should take a CAIP-19 string or an `AssetId`-shaped one is still open                              | —                        |

---

## 1. Native MINA

### Decimals: 9, smallest unit nanomina

- The daemon's currency module sets `let precision = 9` and parses decimal MINA strings against it
  ([MINA `src/lib/currency/currency.ml#L78-L112`](https://github.com/MinaProtocol/mina/blob/96f857bbf4f6ff2e144ea26ce10e247487a07970/src/lib/currency/currency.ml#L78-L112)).
  It defines `unit_to_nano = 1_000_000_000` ("The number of nanounits in a unit"), with
  `to_nanomina_int` and `to_mina_int m = to_int m / unit_to_nano`
  ([`#L329-L335`](https://github.com/MinaProtocol/mina/blob/96f857bbf4f6ff2e144ea26ce10e247487a07970/src/lib/currency/currency.ml#L329-L335)).
  The ledger stores nanomina as an integer. CONFIRMED-CODE.
- `Fee` and `Amount` are built on `Unsigned_extended.UInt64`
  ([`#L890-L919`](https://github.com/MinaProtocol/mina/blob/96f857bbf4f6ff2e144ea26ce10e247487a07970/src/lib/currency/currency.ml#L890-L919)),
  so any one amount is at most 2^64 − 1 nanomina (about 1.8 × 10^10 MINA). The account
  `balance` field uses the same type for every token
  ([MINA `src/lib/mina_base/account.ml#L256-L258`](https://github.com/MinaProtocol/mina/blob/96f857bbf4f6ff2e144ea26ce10e247487a07970/src/lib/mina_base/account.ml#L256-L258)).
  CONFIRMED-CODE.
- Mina's Rosetta implementation reports the native currency as `{ symbol = "MINA"; decimals = 9 }`
  ([MINA `src/lib/rosetta_lib/amount_of.ml#L43-L47`](https://github.com/MinaProtocol/mina/blob/96f857bbf4f6ff2e144ea26ce10e247487a07970/src/lib/rosetta_lib/amount_of.ml#L43-L47)).
  CONFIRMED-CODE.

### SLIP-44 coin type: 12586

- The registry file has the row `| 12586 | MINA | Mina |`
  ([SLIP-0044 line 1358 at `570ed55b`](https://github.com/satoshilabs/slips/blob/570ed55b7fde158f1116be34fc2faa35dada5912/slip-0044.md?plain=1#L1358)).
  CONFIRMED-REGISTRY.
- Mina's own docs give the same index (`12586`, hardened `0x8000312a`, symbol `MINA`) and the
  derivation path `m / 44' / 12586' / account' / 0 / 0`
  ([docs2 `docs/node-developers/bip44.mdx`](https://github.com/MinaProtocol/docs2/blob/5e910fbc293982f63d597a0821b7c8bffb2aa63c/docs/node-developers/bip44.mdx)).
  The page gives one path and no separate path for devnet. CONFIRMED-DOCS.
- The same registry reserves coin type `1` for "Testnet (all coins)"
  ([line 34](https://github.com/satoshilabs/slips/blob/570ed55b7fde158f1116be34fc2faa35dada5912/slip-0044.md?plain=1#L34)).
  That rule is for BIP-44 key derivation. It doesn't decide CAIP-19 asset names, and §3 shows the
  namespace precedent uses the mainnet coin type on a testnet chain.

---

## 2. Mina's custom token model

### Every account is a (public key, token ID) pair

- MIP-4 (zkApps): "accounts are identified by a public key and token id pair". It adds a
  `token_symbol` field, "a string, up to six characters long", and says "New `token_id`s are derived
  from the addresses of the smart contract that defines them"
  ([MIP-4 `MIPS/mip-0004-zkapps.md#L46-L56`](https://github.com/MinaProtocol/MIPs/blob/5aa77c8911e7af39ff9c594175226b171ed8166d/MIPS/mip-0004-zkapps.md?plain=1#L46-L56)).
  CONFIRMED-DOCS.
- The daemon's account record has `token_id`, `token_symbol` and `balance`, and **no decimals field**
  ([MINA `account.ml#L256-L258`](https://github.com/MinaProtocol/mina/blob/96f857bbf4f6ff2e144ea26ce10e247487a07970/src/lib/mina_base/account.ml#L256-L258)).
  CONFIRMED-CODE.

### How a token's identity is derived

- **Native MINA is token ID `1`.** The daemon's comment explains the choice: "Chose 1 for consistency for the old uint64
  based token IDs"
  ([MINA `src/lib/mina_base/account_id.ml#L73-L75`](https://github.com/MinaProtocol/mina/blob/96f857bbf4f6ff2e144ea26ce10e247487a07970/src/lib/mina_base/account_id.ml#L73-L75)).
  o1js agrees: `TokenId.default` returns `Field(1)`
  ([O1JS `src/lib/mina/v1/account-update.ts#L636-L646`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/mina/v1/account-update.ts#L636-L646)).
  Its base58 form, which the GraphQL API takes, is
  `wSHV2S4qX9jFsLjQo8r1BsMLH2ZRKsZx6EJd1sbozGPieEC4Jf`
  ([O1JS `src/lib/mina/v1/graphql.unit-test.ts#L82`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/mina/v1/graphql.unit-test.ts#L82)).
  CONFIRMED-CODE.
- **A custom token's ID is a hash of its owner's account ID.** In the daemon:
  `derive_token_id ~owner = Random_oracle.hash ~init:Hash_prefix.derive_token_id (pack_input (to_input owner))`,
  where `owner` is the (compressed public key, token ID) pair of the **token owner** account
  ([MINA `account_id.ml#L128-L135`](https://github.com/MinaProtocol/mina/blob/96f857bbf4f6ff2e144ea26ce10e247487a07970/src/lib/mina_base/account_id.ml#L128-L135);
  prefix `"MinaDeriveTokenId"` at
  [MINA `src/lib/hash_prefixes/hash_prefixes.ml#L125`](https://github.com/MinaProtocol/mina/blob/96f857bbf4f6ff2e144ea26ce10e247487a07970/src/lib/hash_prefixes/hash_prefixes.ml#L125)).
  In o1js: `TokenId.derive(tokenOwner, parentTokenId = Field(1))` =
  `hashWithPrefix(prefixes.deriveTokenId, packToFields({tokenOwner, parentTokenId}))`
  ([O1JS `account-update.ts#L642-L645`](https://github.com/o1-labs/o1js/blob/a428f9b97817177ef4b5868cf148c0ac67a1048d/src/lib/mina/v1/account-update.ts#L642-L645)).
  A token is therefore identified by the address that owns it. If the owner account is itself
  a token account, that token's ID goes in too, so tokens can nest. CONFIRMED-CODE.
- **The symbol is not an identity.** The docs say: "Uniqueness is not enforced for token names
  because the public key of the manager account is used to derive a unique identifier for each
  token" ([docs2 `custom-tokens.mdx`](https://github.com/MinaProtocol/docs2/blob/5e910fbc293982f63d597a0821b7c8bffb2aa63c/docs/zkapps/writing-a-zkapp/feature-overview/custom-tokens.mdx)).
  CONFIRMED-DOCS.

### Token owner (manager) contracts

- The token owner is a zkApp. The docs say it is "the only account that can" mint, burn and
  approve sends, and that "Sending tokens between two accounts must be approved by a zkApp"
  ([docs2 `custom-tokens.mdx`](https://github.com/MinaProtocol/docs2/blob/5e910fbc293982f63d597a0821b7c8bffb2aa63c/docs/zkapps/writing-a-zkapp/feature-overview/custom-tokens.mdx)).
  At the protocol level, MIP-4's `may_use_token` field (`No`, `Parents_own_token`,
  `Inherit_from_parent`) is the permission an account update needs to touch a custom token. The
  owner's account update is the parent of that update in the call forest, and it can assert over the
  whole subtree
  ([MIP-4 `#L140-L160`](https://github.com/MinaProtocol/MIPs/blob/5aa77c8911e7af39ff9c594175226b171ed8166d/MIPS/mip-0004-zkapps.md?plain=1#L140-L160)).
  o1js packages this as the `TokenContract` class (`approveBase`, `internal.mint/burn/send`).
  CONFIRMED-DOCS.

### The fungible-token standard

- The standard is o1Labs RFC 14, _Fungible Token Standard on Mina_
  ([`o1-labs/rfcs@eeb8070c` `0014-fungible-token-standard.md`](https://github.com/o1-labs/rfcs/blob/eeb8070c9901c611c9a557464022bbf9237900b9/0014-fungible-token-standard.md)).
  It calls for one shared token contract for every token, with minting policy moved into a
  separate admin contract. Its stated reason is that in Mina's off-chain execution model,
  integrating with a contract means "having access to the code of that contract, and running it in
  your own environment". Balances live in the ledger's token accounts, not in contract state.
  It is an RFC in o1Labs' repo. **No MIP covers fungible tokens**: the MIPs directory holds MIPs
  1 and 3–9, and none of them is a token standard
  ([MIPs `MIPS/`](https://github.com/MinaProtocol/MIPs/tree/5aa77c8911e7af39ff9c594175226b171ed8166d/MIPS)).
  CONFIRMED-DOCS.
- The reference implementation is `MinaFoundation/mina-fungible-token` (npm `mina-fungible-token`,
  latest `1.1.0`, last modified 2024-12-13). It keeps **`decimals` as `UInt8` zkApp state**, set once
  in `initialize(admin, decimals, startPaused)`
  ([`FungibleToken.ts#L50-L51`, `#L99-L111` at `8dbf75a4`](https://github.com/MinaFoundation/mina-fungible-token/blob/8dbf75a4a50d236d9a9ab137a7fd451bafddae41/FungibleToken.ts#L50-L111)).
  Its README still says "currently a beta … awaiting an audit". **The GitHub repository is now
  archived** (repo metadata, read 2026-10-27). Mina's docs still point to it as "the fungible token
  standard". CONFIRMED-CODE / CONFIRMED-DOCS. _When_ it was archived, and whether a successor exists:
  UNCONFIRMED. o1js `main` contains no `FungibleToken` class.
- **Rosetta gives every custom token the symbol `MINA+` and 9 decimals**, with the token ID as
  metadata ([MINA `amount_of.ml#L49-L59`](https://github.com/MinaProtocol/mina/blob/96f857bbf4f6ff2e144ea26ce10e247487a07970/src/lib/rosetta_lib/amount_of.ml#L49-L59)).
  It does this whatever the token's own `decimals` state says. That is an interface default, not the
  token's scale. Anything reading a custom token's decimals has to read the token contract.
  CONFIRMED-CODE.

### How a Mina token would be named in CAIP-19, if a profile existed

No profile exists, so this is a projection, not a fact. The pattern other account-model chains use is
a chain-specific asset namespace whose reference is the token's on-chain identity:

- Solana uses `solana:<genesis>/token:<mint>`
  ([namespaces `solana/caip19.md`](https://github.com/ChainAgnostic/namespaces/blob/463bae5625aaa8c184ccc74f1db457da8a43e72b/solana/caip19.md)).
- Stellar has both: `slip44:148` for the native asset, and issuer-qualified `asset:` and contract
  `sep41:` namespaces for issued assets
  ([namespaces `stellar/caip19.md`](https://github.com/ChainAgnostic/namespaces/blob/463bae5625aaa8c184ccc74f1db457da8a43e72b/stellar/caip19.md)).

On Mina the protocol's own identity for a token is the **token ID**. A profile would most likely
spell it as base58 under a Mina-specific namespace, e.g. `mina:mainnet/token:<base58 tokenId>`. That
fits CAIP-19's `asset_reference` grammar `[-.%a-zA-Z0-9]{1,128}`. The token owner's `B62q…`
address would be a weaker reference, because it doesn't capture a nested token's parent ID.
The namespace name (`token`, `fungible`, …) is unknown until someone writes the profile.

---

## 3. CAIP status for Mina

- **CAIP-2: yes, Draft.** `mina/caip2.md`, by the Mina Foundation, created 2023-09-15, status Draft,
  discussion in [namespaces PR #91](https://github.com/ChainAgnostic/namespaces/pull/91) (merged).
  [PR #111](https://github.com/ChainAgnostic/namespaces/pull/111) (merged) only updated doc links.
  The references are **human-readable network names** (`mainnet`, `devnet`, `testnet`, codenames
  such as `berkeley`, joined with `-`), validated by `^mina:[a-z]{1,12}?(-[a-z]{1,12})?$`. **The
  references are names, not a genesis hash.** The resolution method is the daemon's GraphQL
  `networkID` field
  ([`mina/caip2.md`](https://github.com/ChainAgnostic/namespaces/blob/463bae5625aaa8c184ccc74f1db457da8a43e72b/mina/caip2.md)).
  The spike confirmed that field live on both networks on 2026-10-16
  ([spike §3](zk-note-verifier-spike.md#3-caip-2)). CONFIRMED-REGISTRY.
- **CAIP-10 and CAIP-19: no.** The `mina/` directory holds only `README.md` and `caip2.md`
  ([tree](https://github.com/ChainAgnostic/namespaces/tree/463bae5625aaa8c184ccc74f1db457da8a43e72b/mina)).
  The README's `requires` lists only `CAIP-2`. A search of both ChainAgnostic repos for "mina" finds
  no other PR, open or closed, in `namespaces` and none at all in `CAIPs` (`gh pr list --search mina`,
  2026-10-27). CONFIRMED-REGISTRY.
- **The `slip44` asset namespace fits native MINA.** CAIP-19's `asset_type` is
  `chain_id + "/" + asset_namespace + ":" + asset_reference`. CAIP-19 itself says SLIP-44 "offers a
  registry for native fungible tokens across namespaces" and is "the only cross-chain/multi-namespace
  standard incorporated into CAIP"
  ([`CAIPs/caip-19.md`](https://github.com/ChainAgnostic/CAIPs/blob/f46c0fe052eb80875dd94bbb39af16bb96ad6c1a/CAIPs/caip-19.md)).
  CAIP-20 (Draft) defines the namespace. The reference "is an unsigned integer in decimal
  representation and corresponds to `index` of SLIP44"
  ([`CAIPs/caip-20.md`](https://github.com/ChainAgnostic/CAIPs/blob/f46c0fe052eb80875dd94bbb39af16bb96ad6c1a/CAIPs/caip-20.md)).
  Because `slip44` is defined in a CAIP rather than a per-namespace profile, using it doesn't need
  a Mina CAIP-19 profile. **`mina:mainnet/slip44:12586` is well-formed and uses only registered
  parts.** CONFIRMED-REGISTRY.
- **Testnets keep the mainnet coin type in namespace practice.** Stellar's profile gives
  `stellar:pubnet/slip44:148` and `stellar:testnet/slip44:148`
  ([`stellar/caip19.md`](https://github.com/ChainAgnostic/namespaces/blob/463bae5625aaa8c184ccc74f1db457da8a43e72b/stellar/caip19.md)).
  CAIP-19's own examples are all mainnets, so nothing says a testnet should use coin type `1`.
  `mina:devnet/slip44:12586` follows the only precedent there is. CONFIRMED-REGISTRY.

---

## 4. Stablecoins and bridges on Mina

### Fiat-backed stablecoins: none

- **Circle USDC: not on Mina.** Circle's contract-address page lists 38 mainnet chains for native USDC.
  Mina is in neither the mainnet nor the testnet table
  ([developers.circle.com/stablecoins/usdc-contract-addresses](https://developers.circle.com/stablecoins/usdc-contract-addresses),
  read 2026-10-27). CONFIRMED-DOCS.
- **Tether USDT: not on Mina.** Tether's supported-protocols page lists 13 current protocols and
  5 deprecated ones. Mina is in neither list
  ([tether.to/en/supported-protocols](https://tether.to/en/supported-protocols/), read 2026-10-27).
  CONFIRMED-DOCS.
- **Bridged USDC ("wUSDC"): announced, then not delivered.** On 2025-01-31 the Mina Foundation
  wrote about a Sotatek-built "wETH Ethereum<>Mina bridge". It said "work has already begun on adding
  wUSDC support to the bridge", and that a mainnet deployment was expected after a Zellic audit
  ([minaprotocol.com/blog/mina-foundation-product-priorities-q1-2025](https://minaprotocol.com/blog/mina-foundation-product-priorities-q1-2025)).
  On 2025-04-17 the Foundation wrote that "deployment of the multi-sig bridge has been canceled in
  favor of the Nori approach"
  ([minaprotocol.com/blog/mina-community-call-recap](https://minaprotocol.com/blog/mina-community-call-recap)).
  No later primary source mentions wUSDC. CONFIRMED-DOCS for the cancellation. Whether any wUSDC
  was ever deployed: UNCONFIRMED, and nothing suggests it was.

### Mina-native stablecoin: zkUSD / Fizk, devnet only

- zkUSD is "a private, algorithmic stablecoin protocol built on the Mina Protocol". Users mint
  it against MINA locked in per-user vaults, priced by oracle proofs
  ([`zkUSD-Protocol/core@2142a531` README](https://github.com/zkUSD-Protocol/core/tree/2142a531c3)).
  Its last commit is 2024-12-19, and the org's most recent push is 2025-08-08. CONFIRMED-CODE.
- The team's last status post (forum, 2025-04-07): "The beta readiness deliverables have all been
  completed … Our devnet node is operational", and "There is still a lot of work to do before we
  can feasibly deploy to mainnet". The same post refers to the project as "Fizk". The grant proposal
  put mainnet deployment and the security audit outside its scope
  ([forums.minaprotocol.com/t/6786](https://forums.minaprotocol.com/t/zkusd-beta-readiness-completing-the-protocol-for-full-operational-testing/6786)).
  The Foundation listed zkUSD as "An algorithmic zk stablecoin for Mina" among supported projects on
  2025-06-13
  ([minaprotocol.com/blog/a-new-chapter-for-the-mina-ecosystem](https://minaprotocol.com/blog/a-new-chapter-for-the-mina-ecosystem)).
  o1Labs' Q1 2026 (2026-04-16) and Q2 2026 (2026-07-21) ecosystem updates don't mention it
  ([q1](https://minaprotocol.com/blog/q1-2026-update),
  [q2](https://minaprotocol.com/blog/q2-2026-mina-ecosystem-update)). CONFIRMED-DOCS.
  **A mainnet launch: UNCONFIRMED.** No primary source announces one, and the repos have been quiet
  for over a year.

### Bridges

- **Nori (Ethereum → Mina, zk light client): staging on devnet, ETH only.**
  - Its Mina contract "enabl[es] users to mint nETH tokens", and the token is the standard
    `FungibleToken`
    ([`Nori-zk/nori-bridge-sdk@2a73bdf8` `contracts/mina/README.md`](https://github.com/Nori-zk/nori-bridge-sdk/blob/2a73bdf88f9326478dfab02235afc2491b2441a8/contracts/mina/README.md)).
  - The SDK's baked-in `env` has **only a `mina.staging` entry**, pointed at
    `mesa-rc.minaprotocol.com` with network ID `devnet`. There is no `production` entry
    ([`contracts/mina/src/env.ts`](https://github.com/Nori-zk/nori-bridge-sdk/blob/2a73bdf88f9326478dfab02235afc2491b2441a8/contracts/mina/src/env.ts)).
  - Its production runbook says "Two open items must be resolved **before** mainnet deploy", one
    being that "No verifier is currently wired in to `unlockTokens`"
    ([`contracts/DEPLOYMENT.md`](https://github.com/Nori-zk/nori-bridge-sdk/blob/2a73bdf88f9326478dfab02235afc2491b2441a8/contracts/DEPLOYMENT.md)).
  - The changelog for 2026-09-23 removes Aligned Layer, the planned Mina → Ethereum leg, because it
    "is deprecated". Since then `unlockTokens()` "always reverts"
    ([`CHANGELOG.md`](https://github.com/Nori-zk/nori-bridge-sdk/blob/2a73bdf88f9326478dfab02235afc2491b2441a8/CHANGELOG.md)).
  - o1Labs reported on 2026-07-21 that "The Nori bridge is undergoing an audit with zellic"
    ([q2 update](https://minaprotocol.com/blog/q2-2026-mina-ecosystem-update)).

  CONFIRMED-CODE / CONFIRMED-DOCS. There is no stablecoin leg in the SDK.

- **Sotatek / Mina Foundation multi-sig bridge**: cancelled (above).
- **"zkBridge"**: no primary source names a Mina zkBridge that is distinct from Nori. UNCONFIRMED.
- **Zeko** (a Mina L2) "has officially launched on mainnet", per the
  [Q2 2026 update](https://minaprotocol.com/blog/q2-2026-mina-ecosystem-update). Zeko assets
  are not Mina L1 assets, and no stablecoin on Zeko was found. UNCONFIRMED.
- **Token issuance exists on mainnet, just not stablecoins.** The Foundation reported on 2025-01-31
  that "The Mina Tokens launchpad is now on mainnet"
  ([product priorities](https://minaprotocol.com/blog/mina-foundation-product-priorities-q1-2025)).
  The Q1 2026 update names Solis as "the first DEX to go live on mainnet". CONFIRMED-DOCS.

---

## 5. What this means for TOON

### 5.1 Is `mina:mainnet/slip44:12586`, 9 decimals, correct for native MINA?

**Yes**, as a CAIP-19 string, and it is the most defensible choice there is:

- every component has an owner who has registered it: the `mina` namespace for the chain part,
  CAIP-20 for `slip44`, and SLIP-0044 for `12586`;
- it matches the native-asset form CAIP-19's own examples use (`eip155:1/slip44:60`,
  `cosmos:cosmoshub-3/slip44:118`), and the form Stellar's profile uses on mainnet and testnet;
- 9 decimals matches the daemon (`precision = 9`) and Rosetta (`decimals = 9`).

The vectors should say, as triage intends, that the string is **TOON's composition from
registered parts**, not a `mina` CAIP-19 profile entry. If the Mina Foundation later publishes a
profile that names native MINA differently, the string would have to change.

### 5.2 One open question about form, not content

The connector already has an asset identity, and **it is not CAIP-19**. ADR 0071's `AssetId`
(`crates/connector-domain/src/asset.rs`) is `<chain>:<token>`, e.g. `evm:0x…` or `solana:<mint>`,
as used in `local/dealing/connector-b.toml`'s `[[tokens]]`. It is a closed enum over the two
settlement chains, and the module's doctrine is that it **carries no decimals** ("scale is not
price"). ADR 0074's greeting entries likewise carry a CAIP-2 `network` and an `asset` that is the
token address or mint, not a CAIP-19 string. ADR 0080 decision 7 makes a zk edge cross "a
denomination at a declared rate (0071)". 0071's rate is keyed by an ordered pair of `AssetId`s.
At the same time, a `mina:` asset in `[[tokens]]` stays refused by name. So for #1522 / #1523:

1. **Pick one spelling per surface.** In the greeting's `accepts[]` the natural split, following
   0074, is `network: "mina:mainnet"` plus an `asset` field. For native MINA, `slip44:12586` or the
   full CAIP-19 string are the only registered names. In the zk-note table and in the rate the
   forward crosses at, the asset has to be something the rate machinery can key on. Either
   `AssetId` gains a zk-note-only Mina arm, or the zk-note table carries its own rate to the
   next hop's `AssetId`. Neither is decided anywhere yet.
2. **Treat decimals as an assertion, not an input.** Following `asset.rs` and
   `[settlement] decimals`, "9" belongs in the zk-note table as a boot-time assertion and in the
   vectors as a pinned fact. It shouldn't be a scale the arithmetic consumes. The scale goes into
   the declared rate, as it does for every other boundary. Unlike EVM and Solana, there is nothing
   on chain to assert native MINA's decimals against, because it is a protocol constant. The
   assertion is against the constant.
3. ADR 0071 decision 3 puts a token's quote "on its own settlement chain". Mina has no settlement
   backend here and no on-chain TWAP for MINA in this design. So the MINA→next-hop rate is
   operator-declared, or it comes from somewhere 0071 doesn't yet name. The ADR should say which.

### 5.3 Does native-only paint the design into a corner?

**Not for the connector.** The reasons, from the token model in §2:

- **A vault holds one asset, fixed by its own account.** A zkApp holding native MINA uses its
  `(address, tokenId = 1)` account. Holding a custom token means a different ledger account,
  `(vault address, derive(tokenOwner))`, and every send into or out of it needs the token owner
  contract's approval (`may_use_token`, `approveBase`). A token vault is therefore a **different
  zkApp with different proof programs and a different verification key**, not a setting on the same
  one. The connector already pins exactly those things: vault address, verification key hash, network.
- **The allowance proof doesn't touch the token.** Per #1511, the request proof shows that some
  unspent note under a known root covers tier `b`. The binding tag commits to zk edge identity,
  session key, `b` and expiry. None of that names a token ID. Deposits, withdrawals, escape and
  collection do, and all of them are out of this repository's scope. So the **vault's proof
  programs** (outside this repo) would need the token owner's approval flow, but the connector's
  admission, journal and allowance arithmetic would not change.
- **Amounts fit.** Every Mina balance, native or custom, is `uint64` in base units, so tiers and
  cumulative amounts don't need wider integers for a token vault.

What would paint it into a corner, and should be avoided now:

- **Hardcoding** `slip44:12586` or 9 decimals in code or in the vectors' _structure_. They should
  be values of the zk-note table: the asset string and a decimals assertion, with native MINA the
  only accepted value for now. A refusal by name can keep the scope tight, as `mina:` is refused
  elsewhere.
- **A note commitment without an asset**, _if_ one vault could ever hold more than one asset.
  The domain separation (`TAG_*`) and the note/commitment layout are being fixed in #1521. If
  multi-asset vaults are ever wanted, the asset (token ID) has to be inside the leaf or
  commitment from the start. If one-asset-per-vault is the rule, say so in ADR 0080 and nothing
  needs to go in. **This is the only decision in reach today that a stablecoin could later make
  expensive.** It belongs to #1521 and the vault's proof program, not the asset string.
- **Assuming a token's decimals come from the protocol.** They come from the token contract's state
  (`FungibleToken.decimals`), and Rosetta reports a misleading `9` for every custom token. A
  future token vault would have to read and pin the contract's value.

### 5.4 Things in ADR 0080, #1511 and #1513 that look wrong or loose

- **#1511, Out of Scope: "Tokens on Mina, including USDC."** There is no USDC on Mina: not Circle's,
  not bridged (the wUSDC plan was cancelled with its bridge), and no live Mina-native stable. The
  wording reads as if one exists and is merely deferred. Suggest "Tokens on Mina. No stablecoin is
  live on Mina mainnet as of 2026-10 (see this note)."
- **#1511: "The asset is whatever the vault holds."** The triage on #1513 reads this as "so the asset
  is native MINA". That follows only if the vault holds native MINA. The vault is out of scope and
  hasn't been written. ADR 0080 or #1522 should say explicitly that **the vault holds native MINA**,
  so the asset string is a fact about the pinned vault and not an inference.
- **ADR 0080 decision 7** doesn't name the asset at all, and doesn't say how the
  "declared rate (0071)" is keyed when `mina:` assets are refused in `[[tokens]]`. See §5.2. It
  should cite the asset string and its decimals once #1513 settles them.
- **#1513 triage** describes the CAIP-19 string as the "zk-note table" asset. Whether the table takes
  a CAIP-19 string or an `AssetId`-shaped `mina:…` string is the open question in §5.2. The string
  is right; the surface it goes on isn't decided.

---

## Unconfirmed, and why

- **The archive date of `mina-fungible-token`, and any successor standard.** The repo is archived, but
  the API doesn't expose when it was archived. No successor was found in o1js or Mina's docs.
- **zkUSD / Fizk mainnet status after April 2025.** No primary source past the forum's 2025-04-07
  post. A launch could have happened without one, but the code repos show no activity consistent
  with it.
- **Nori's mainnet date.** The Zellic audit was reported in progress on 2026-07-21. The SDK at
  2026-09-29 has no production config and lists blockers. Nori's audit-findings repo
  (`Nori-zk/audits`) wasn't read.
- **Any stablecoin on Zeko** or on another Mina L2. Not searched beyond o1Labs' own updates.
- **Third-party "bridged USDC" on Mina** outside the Foundation's channels. None was found, and an
  absence can't be proven from issuer pages.
