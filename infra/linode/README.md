# `infra/linode/` — the retired self-hosted chain box

> **There is no box here any more.** This directory holds exactly one live
> artifact: [`endpoints.json`](./endpoints.json), the public-chain devnet
> endpoint record. Everything that provisioned a machine has been deleted.

## What was here, and why it went

`infra/linode/` provisioned the **self-hosted chain box** — an anvil EVM node
(chain-id 31337), a `solana-test-validator`, the multichain faucet, an
nginx→Mina passthrough, and nginx + Let's Encrypt TLS in front of them, served
at `evm-rpc.` / `solana-rpc.` / `solana-ws.` / `faucet.` / `mina.<DOMAIN>`. It
was never a connector box: `infra/linode-relay/` and `infra/linode-store/`
provision the two machines that actually serve devnet, and `infra/linode-faucet/`
provisions the faucet.

That box was deleted in the public-chain cutover — commit `44b15bdc`,
2026-07-19, "docs: public-chain cutover — retire self-hosted devnet chain
endpoints (#374)". Devnet settles on **public chains** now: Base Sepolia
(`evm:84532`) and public Solana devnet. The subdomains it
answered on are gone; `*.devnet.toonprotocol.dev` is a wildcard A-record to the
registrar's parking host, so `evm-rpc.` and `mina.` return a 301 to
`toonprotocol.dev` and fail TLS.

The provisioning outlived the box, and ran zero times after it. Its scripts
(`bootstrap.sh`, `devnet.sh`, `firewall.sh`, `init-letsencrypt.sh`),
its compose overlay, its `.env.example` and its four nginx templates, together
with their sole caller `.github/workflows/devnet-deploy.yml`, were deleted.
`devnet-deploy.yml` was `workflow_dispatch`-only behind the reviewer-gated
`devnet` GitHub Environment and last ran **2026-06-23**, four weeks _before_ the
cutover. Its `destroy` action deleted a Linode by label (`toon-devnet`), which
is a live footgun to leave dispatchable against a label that should no longer
resolve. Git history has all of it if a self-hosted chain box is ever wanted
again; recreating it from this directory would not be a restoration.

Two deterministic mock tokens belonged to that box and **do not exist on any
live chain**: the anvil `MockERC20` USDC `0x5FbDB2315678afecb367f032d93F642f64180aa3`
(with `TokenNetworkRegistry` `0xe7f1725E…`) and the Solana mock USDC SPL mint
`H8HSreUF2s8r8hem4qMttE3bWYCpFuh71jbuos5bA77H`. Both addresses are still correct
for the **local** disposable chains (`docker-compose.yml`, `local/`), which
reproduce them from genesis on every run — that is a different thing at the same
address, and `infra/solana/usdc-authority.json` is the authority for the local
mint only.

## What survives: `endpoints.json`

[`endpoints.json`](./endpoints.json) is **live and hand-maintained**. It records
the public devnet's chain endpoints, token addresses and program ids, and is the
canonical answer to "what does a TOON node point at on devnet". It is read at
cited as the source of truth by `docs/usdc-cross-chain-settlement.md` and
`docs/operators/peer-channel-migration.md`. Its readers used to include a pile of
Mina tooling; that went with Mina itself (ADR 0065-mina), and so did its `mina` block.

Nothing generates it any more — `devnet.sh endpoints`, which used to, is gone
with the box. **Edit it by hand** when a devnet address changes.

A second copy of that generator outlived the first, and this paragraph did not
know about it: `infra/devnet-manage.sh` kept an `endpoints` verb until issue
#1135, printing a JSON document frozen at 2026-06-23. It queried three box
labels that no longer resolve, named the deleted self-hosted chains and their
mock tokens, the retired `proxy.store.` edge, and — the reason it was finally
found — a **Solana payment-channel program id that has never been deployed to
public devnet**, disagreeing with the `solana.programId` above and with both
box configs. It is deleted, and
`crates/connector-settlement-solana/tests/solana_program_ids.rs` now fails the
build if any committed file names a Solana program id that is neither the
public-devnet deploy nor the disposable local validator's. Since ADR 0053 binds
the program id into a claim's signed message, a stale one here is not a
mislabelling for long.

The EVM half of this file drifted the same way and was caught the same way, a
month later. `tokenNetworkUsdc` is **derived**: it is whatever
`registryAddress.getTokenNetwork(tokenAddress)` answers on chain, so the three
move together or the document is lying. The 2026-08-06 ERC-2771 cutover
(#695/#811, [`docs/evm-deployment.md`](../../docs/evm-deployment.md)) repointed
`registryAddress` in both blocks and left `tokenNetworkUsdc` naming the
2026-07-18 contract it replaced. It stood wrong for three weeks, and nothing on
the fleet was affected for a reason worth stating: a connector is configured
with the **registry** (`[settlement.evm] contract_address`) and resolves the
`TokenNetwork` itself at boot, so it never reads this key. Only the audience
this file exists for — someone configuring themselves from it — was pointed at
a contract the live registry does not resolve. Two guards now stand where
memory did: `devnet_configs_load.rs`'s
`the_public_endpoints_document_names_the_fleets_live_evm_deployment` holds both
blocks to the fleet's addresses on every push, and
`.github/workflows/base-sepolia-redeem-gate.yml` asks Base Sepolia itself
whether the registry still resolves to the address published here.
`docs/evm-deployment.md`'s repoint checklist names this file now; it did not
then, which is the whole of why the repoint half-finished.

## Where the live devnet is documented

| Thing                                             | Where                                                               |
| ------------------------------------------------- | ------------------------------------------------------------------- |
| Devnet endpoints, tokens, program ids             | [`endpoints.json`](./endpoints.json)                                |
| The two connector boxes (now fixtures — ADR 0068) | `infra/linode-relay/`, `infra/linode-store/`                        |
| The faucet box                                    | `infra/linode-faucet/`, `docs/operators/faucet-box-bringup.md`      |
| Fleet lifecycle (provision / DNS / destroy)       | `infra/devnet-manage.sh`                                            |
| Fleet CI                                          | `.github/workflows/fleet-ops.yml` (faucet only), `fleet-health.yml` |
| Disposable local chains                           | `docker-compose.yml`, `local/`, `local/README.md`                   |
