# USDC settlement across all chains (design)

> Status: design + decomposition (historical). Implementation tracked in the linked tickets.
> ⚠️ **Note (2026-08-27):** every Mina section below describes code that no longer
> exists. [ADR 0065](adr/0065-mina-leaves-the-repository.md) removed Mina from this
> repository — the zkApp, the token, the browser faucet dApp and the faucet's Mina leg
> are all deleted, and `endpoints.json` no longer carries a `mina` block. Those sections
> are kept as the record of a design that shipped and was then retired; read them as
> history, not as a description of the tree.
> ⚠️ **Note (2026-07-19):** the self-hosted devnet chains referenced below
> (anvil `0x5FbDB2…`, self-hosted Solana validator mint `H8HSreUF…`) are
> **deleted** — the devnet now settles on public chains (Base Sepolia, public
> Solana devnet); see
> [`infra/linode/endpoints.json`](../infra/linode/endpoints.json) for live
> values. The addresses below remain valid for the **local** docker-compose
> chains only.
> Driver: make the shared devnet (and protocol) settle **USDC** on every supported
> chain — EVM, Solana, **and Mina** — with one canonical decimal scale.

## Goal & current state

| Chain      | USDC today                                                                                                                                                               | Decimals | Gap                                                                  |
| ---------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | -------- | -------------------------------------------------------------------- |
| **EVM**    | `MockERC20` `0x5FbDB2…` + `TokenNetwork` (EIP-712 claims)                                                                                                                | **6** ✅ | migrated 18→6 in `DeployLocal.s.sol`; address unchanged, suite green |
| **Solana** | program is fully SPL-aware (channel state stores `token_mint`, vault is an SPL account); devnet mock-USDC mint `H8HSreUF…` created by `infra/solana/create-usdc-mint.sh` | **6**    | ✅ done                                                              |
| **Mina**   | `PaymentChannel` zkApp settles **native MINA**; `tokenId_` stored but unused. **USDC token zkApp (6dp) added** (`src/usdc-token.ts`, `mina-fungible-token`) ✅           | 6        | remaining: make `PaymentChannel` deposit/settle token-aware (#191)   |

## Decisions (locked)

1. **6 decimals everywhere.** Real-USDC standard. Solana is already 6; migrate the
   EVM mock 18→6; Mina USDC token is created with 6. With all chains on one scale,
   a claim's `transferredAmount` is the same integer base unit (10⁻⁶ USDC)
   everywhere — **no cross-chain decimal normalization needed.**
2. **Mina token = `mina-fungible-token` standard** (audited Mina Foundation lib,
   v1.1.0, o1js ^2.2.0) — not hand-rolled. We deploy it as the USDC token-owner.

## Canonical unit

All three chains settle in **base units of a 6-decimal USDC** (1 USDC = 1_000_000
base units). The connector's claim payloads (`transferredAmount` in
`btp-claim-types.ts`) are already strings of integer base units per chain — once
decimals are aligned they need no scaling. Add a startup assertion that each
configured token's `decimals == 6` so a misconfig fails loud instead of mis-settling.

**Shipped for the Rust connector (issue #564).** `[settlement] decimals` is honoured as
exactly that assertion, not as a scale factor: `EvmSettlementBackend::connect` reads the
configured token's own `decimals()` and refuses to start when it disagrees with the
config file, naming both values. Nothing multiplies or divides by `decimals` anywhere on
the value path — because of the rule above, there is nothing to normalize — so the only
honest way to honour the field is to refuse a node whose declared scale is a lie.

## EVM — migrate mock USDC 18 → 6 ✅ done

Done in `packages/contracts/script/DeployLocal.s.sol` (the devnet USDC peers settle
with): `MockERC20("USD Coin","USDC",18)` → `6` and `tokensPerPeer 10000*10**18` →
`10**6`. Verified on a live anvil: USDC stays at the deterministic address
`0x5FbDB2315678afecb367f032d93F642f64180aa3` (it's deployer+nonce-derived, not
constructor-derived), `decimals()==6`, peers funded 10k USDC; `forge test` 63/63 green.

Deliberately **out of scope** (left at 18 / unchanged, with reasons):

- `src/TokenNetworkRegistry.sol:81` `maxChannelDeposit = 1_000_000 * 10**18` — this is
  a single **raw-unit** cap applied across all token decimals. The mixed-decimals
  integration test (`TokenNetwork.integration.t.sol`, USDC 6 / DAI 18 / USDT 6)
  deposits `1000 * 10**18` DAI through a registry-created network, so dropping the
  cap to `10**6` would make it revert. A per-decimals cap is a separate concern.
- The decimal-agnostic unit tests use a generic 18-dp "TEST" token internally; they
  exercise `TokenNetwork` mechanics (which are unit-agnostic) and don't represent USDC.
- `DeployTestnet.s.sol` deploys a distinct **AGENT** token, not USDC — untouched.
- Faucet (`packages/faucet/src/index.js`) parses with the **runtime** token decimals,
  so its drip auto-scaled to 6 with no code change.

## Solana — done

`infra/solana/` ships `usdc-mint.json` (deterministic mint `H8HSreUF…`, 6 dp),
`create-usdc-mint.sh` (idempotent create + treasury), `fund-solana.sh` (SOL + USDC
drip). The program already settles any SPL mint; provider/SDK already take
`tokenMint`. Wire `endpoints.json.solana.tokenMint` (done in the linode overlay).

## Mina — the real work

### Token-owner zkApp ✅ done (#190)

`src/usdc-token.ts` wraps `mina-fungible-token`'s `FungibleToken` +
`FungibleTokenAdmin` as USDC (symbol `USDC`, **decimals 6**, `ONE_USDC` helper).
Verified in `src/usdc-token.test.ts` (deploy at 6dp → admin-authority mint →
transfer; `proofsEnabled:false`). `jest.config.ts` gained a `.js`→CJS transform so
the ESM lib loads. Gotcha encoded: the **mint authority must be a funded account**
(an unfunded admin key breaks account-creation-fee accounting).

Because we **proxy the public Mina devnet** (no self-hosted node), the token zkApp
is deployed **once to public devnet** and its address + derived `tokenId` pinned in
config/`endpoints.json` (`mina.tokenAddress`, `mina.tokenId`). Minting to peers is
done by the admin authority.

### PaymentChannel becomes token-aware

File: `packages/mina-zkapp/src/PaymentChannel.ts`.

- **deposit** (today `depositorUpdate.send({to:this.address, amount})` native, ~L141-152)
  → move the **custom token**: `FungibleToken.transfer(depositor → channel token
account)` under the USDC `tokenId` (the standard lib's `transfer` emits the
  owner-approved account updates).
- **settle** (today `this.send({to, amount})` native, ~L305-308) → token transfers
  of `balanceA`/`balanceB` from the channel's token account to participants.
- The channel zkApp holds a **token account** (its address under the USDC
  `tokenId`); `depositTotal` and balances are now USDC base units.
- **Fee payer stays MINA** — Mina always charges fees in MINA regardless of the
  token settled; the funded treasury pays them. No change to `_feePayer`.

### Invariant: keep `channelHash` native

`channelHash = Poseidon(apex.x, client.x, nonce)` and the **bare-deploy** flow
(`MINA_SKIP_INIT`, `E2E_MINA_ZKAPP_INDEX=98`, the connector's
`Poseidon(apex,client,0)` reproduction) **must not change**. `tokenId` is a channel
**parameter**, not part of identity → store it in state, exclude it from
`channelHash`. Consequence: **one channel per (apex, client, token)**. Verify the
`nonceField` 0→1 settle proof still holds.

### SDK / provider threading

- `mina-payment-channel-sdk.ts` `settleChannel`/`openChannel` thread the token
  (`tokenId` + token-owner address) into the on-chain methods.
- `mina-payment-channel-provider.ts` `MinaProviderConfig.tokenId`/`tokenAddress`
  populated from config; default `'MINA'` path removed for USDC channels.
- `MinaClaimMessage` already carries `tokenId` — assert it matches config.

### Proving budget

Token-aware proofs add constraints (extra account updates to prove). Expect higher
compile + per-tx proving time; keep Mina settlement **nightly, not per-PR**
(matches existing e2e guidance). Re-pin the verification key after the change.

## Devnet wiring (after Mina lands)

- Deploy the USDC `FungibleToken` to public Mina devnet; pin `mina.tokenAddress` /
  `mina.tokenId` in `infra/linode/endpoints.json` (currently `mina.tokenId: null`).
  That file is hand-maintained now — the chain box's `devnet.sh endpoints`
  generator was deleted with the box's provisioning.
- Add a Mina USDC funding path analogous to `fund-solana.sh` / the EVM faucet.
  (DONE, updated for the rate-limited redeploy: the token's mint is
  permissionless-but-recipient-signed, so funding is either the
  `tools/mina/self-mint-usdc.mts` self-mint — wrapped by
  `infra/mina/fund-mina-usdc.sh` — or a treasury TRANSFER for zero-MINA
  recipients; admin-mint is legacy, stock-admin deploys only. The faucet's
  Mina route this once named is gone with Mina itself, ADR 0065.)

## Risks

- **o1js token API correctness** — biggest unknown; validate against
  `mina-fungible-token@1.1.0` with real `PaymentChannel.compile()` + tests.
- **Proving cost / throughput** (Mina's per-block zkApp-tx cap) under settlement bursts.
- **Admin key custody** for the token zkApp (testnet-only key; never reuse mainnet).
- **Per-token channel** requirement — ensure routing/settlement opens a channel
  keyed by token, not just peer.

## Tickets

See the epic and children filed in `toon-protocol/connector` (linked from this
doc's PR). Ordering: EVM-decimals → Mina token zkApp → token-aware PaymentChannel →
SDK/provider threading → connector decimals assertion → devnet deploy/funding → tests.
