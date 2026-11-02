# Robinhood Chain: is x402's batch settlement there, and can a connector settle on it by config alone?

> **Research note, not a decision record.** It decides nothing. It answers one question and checks
> [`robinhood-chain.md`](robinhood-chain.md), which left "probably config-only" open. Every source and
> live read was taken on **2026-11-02**. No transaction was sent to either network.

**Evidence tags** (as in [`x402-pending-chain-bindings.md`](x402-pending-chain-bindings.md)).
CONFIRMED-CODE means read in source at a pin, or read live from a chain's RPC. CONFIRMED-DOCS means read
in the owner's own documents. UNVERIFIED means neither, and says why. **Inference** marks this note's own
reasoning. This note adds **CONFIRMED-RUN**: this repository's real `connector` binary, built at the pin,
was run and printed the result quoted.

**Pinned.** This repository at `08dbc279`; x402 at
[`0cb1a1f0`](https://github.com/x402-foundation/x402/tree/0cb1a1f0f4c2163357e255c824d319674e1db43f) (the
committed bytecode's pin) and `0c58a71c` (`main` today); toon-client at `6cfd3471`; Nitro at `ccc5c828`.
Tools: Foundry 1.7.1, ethers 2.0.14. In commands, `M` is `https://rpc.mainnet.chain.robinhood.com`,
`T` is `https://rpc.testnet.chain.robinhood.com` and `X` is
`0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003`.

---

## The answer

| Question                                                 | Mainnet (4663)                                     | Testnet (46630)                                                                          |
| -------------------------------------------------------- | -------------------------------------------------- | ---------------------------------------------------------------------------------------- |
| `x402BatchSettlement` deployed at the canonical address? | **Yes**                                            | **Yes**                                                                                  |
| Collectors and Permit2 present?                          | Yes                                                | Yes                                                                                      |
| Does the unmodified binary boot on it from config?       | **Yes** (CONFIRMED-RUN, public RPC)                | **Yes** (CONFIRMED-RUN, public RPC)                                                      |
| Full channel lifecycle with no code change?              | **Yes on a local fork of mainnet** (CONFIRMED-RUN) | Not run. The token's and contract's domain and code checks are the same (CONFIRMED-CODE) |
| Lifecycle on the real Nitro sequencer?                   | **UNVERIFIED** (sending was out of scope)          | **UNVERIFIED**                                                                           |

**Config-only: yes. No code change was found to be needed.** The binary has no chain allowlist, and two
peered, unmodified nodes ran open, top-up, paid packets, claim, settle and withdraw in USDG on a local
fork of mainnet. The one thing not shown is a transaction accepted by the live sequencer. Everything
either side of that step was exercised against live Nitro (§3.4).

**Code blockers: none.**

**Needed, none of it code:**

1. A key funded with ETH for gas and with USDG on 4663. There is no USDC to use (see the
   [earlier note](robinhood-chain.md)).
2. A counterparty with its own `[settlement.eip155.4663]` table on the same token (§5). A mismatch is
   `TokenNotShared` at `crates/connector-settlement-evm/src/batch_payer.rs:318`.
3. `asset_eip712_name = "Global Dollar"` and `asset_eip712_version = "1"`. Both are required
   (`crates/connector-config/src/settlement.rs:1023`) and checked at boot.

**Worth doing before relying on it, also not blockers:**

- One owner-authorised send on testnet 46630, which is the only way to close §3.4's gap.
- A misleading refusal message. `crates/connector-cli/src/runtime.rs:449` tells an operator to find the
  version with `cast call <token> 'version()(string)'`, and USDG's `version()` reverts. Text only;
  nothing depends on it.

---

## 1. The deployment

### 1.1 The canonical addresses

The connector binds these as constants, never as config (ADR 0075). `x402BatchSettlement` is
`crates/connector-signer/src/voucher_signature.rs:70`; the collectors and Permit2 are
`crates/connector-settlement-evm/src/batch_payer.rs:98`, `:104` and `:110`. `infra/anvil/seed.sh:76-79`
spells the same four, and x402's own
[`contracts/evm/README.md`](https://github.com/x402-foundation/x402/blob/0c58a71cc2b050f8512e3226bfd0ff18a1fbe750/contracts/evm/README.md)
line 32 gives the batch contract's. CONFIRMED-CODE.

### 1.2 The bytecode against the committed runtime

```bash
cast chain-id --rpc-url $M    # 4663
cast chain-id --rpc-url $T    # 46630
cast code <address> --rpc-url <M or T>
# compared byte by byte with crates/connector-settlement-evm/contracts/x402/<name>.runtime.hex
```

| Contract                                | Size                 | Mainnet            | Testnet            |
| --------------------------------------- | -------------------- | ------------------ | ------------------ |
| `x402BatchSettlement` `0x4020…0003`     | 11,175, as committed | 35 bytes differ    | 35 bytes differ    |
| `ERC3009DepositCollector` `0x4020…0004` | 1,150                | **Byte-identical** | **Byte-identical** |
| `Permit2DepositCollector` `0x4020…0005` | 2,467                | **Byte-identical** | **Byte-identical** |
| Permit2 `0x0000…8BA3`                   | 9,152                | 34 bytes differ    | 35 bytes differ    |
| `SignatureChecker` `0xbA3b…7DA6`        | 0                    | Absent (`0x`)      | Absent (`0x`)      |

**Every differing byte range**, with offsets in bytes:

| Contract              | Range     | Committed (Base Sepolia) | Mainnet             | Testnet             | Why                                                                                    |
| --------------------- | --------- | ------------------------ | ------------------- | ------------------- | -------------------------------------------------------------------------------------- |
| `x402BatchSettlement` | 8004–8006 | `014a34` = 84532         | `001237` = 4663     | `00b626` = 46630    | Cached chain id immutable                                                              |
| `x402BatchSettlement` | 8017–8048 | `fdef8076…4dffce`        | `fa78e9a7…da4a01`   | `a5fa2996…c1dfb9`   | Cached EIP-712 domain separator immutable                                              |
| Permit2               | 6974–6976 | `014a34`                 | `001237`            | `00b626`            | Cached chain id immutable                                                              |
| Permit2               | 6983–7014 | `010f27a9…ab26592c`      | `44868446…85614fad` | `385ef69f…ed24173f` | Cached separator. On mainnet byte 7005 happens to match, hence 34 bytes rather than 35 |

**The separators recompute exactly**, so those ranges hold nothing else:

```bash
cast keccak $(cast abi-encode 'f(bytes32,bytes32,bytes32,uint256,address)' \
  $(cast keccak 'EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)') \
  $(cast keccak 'x402 Batch Settlement') $(cast keccak 1) <chainId> $X)
# 4663  -> 0xfa78e9a7e03abfe74da673d366e9a79c6a0d305f1eb1657a155b1f6c51da4a01
# 46630 -> 0xa5fa2996226868c075117f5f5b6cc75a7a7eba0957efe44cf08bbea158c1dfb9
```

`cast call $X 'eip712Domain()(…)'` answers `"x402 Batch Settlement"`, `"1"`, the chain id and `X` on both
networks. Permit2's `EIP712Domain(name,chainId,verifyingContract)` over `"Permit2"` gives `0x44868446…`
and `0x385ef69f…`, each equal to the live `DOMAIN_SEPARATOR()`. CONFIRMED-CODE.

**`SignatureChecker` is not a blocker.** It is Circle's library, linked by FiatToken and placed only by
the harness and `infra/anvil/seed.sh`. USDG is not FiatToken, and the fork run in §3.3 succeeded without
it. CONFIRMED-RUN.

### 1.3 Who controls it

**Nobody, at the contract level.** The evidence, the same on both networks:

- **The source has no admin.** `contract x402BatchSettlement is EIP712, Multicall, ReentrancyGuardTransient`,
  with a constructor taking no arguments
  ([source](https://github.com/x402-foundation/x402/blob/0cb1a1f0f4c2163357e255c824d319674e1db43f/contracts/evm/src/x402BatchSettlement.sol),
  lines 24 and 185). Its committed ABI has no owner, admin, pause or upgrade function. CONFIRMED-CODE.
- **`owner()`, `admin()`, `implementation()`, `paused()` and `proxiableUUID()`** each revert with `0x`.
- **Not a proxy.** `cast storage $X` reads zero at the EIP-1967 implementation, admin and beacon slots,
  and at slot 0.
- **The one `DELEGATECALL`.** The disassembly has one, no `SELFDESTRUCT` and no `CREATE`. **Inference:**
  it is OpenZeppelin `Multicall`, which delegates to the contract itself.
- **Constants live:** `MIN_WITHDRAW_DELAY()` = 900 and `MAX_WITHDRAW_DELAY()` = 2,592,000.

**Who deployed it.** On testnet the explorer API names the deterministic CREATE2 deployer `0x4e59…956C`
and creation transaction `0x8c189821…6e28`, with the contract unverified on that explorer. On mainnet
**unverified**: the explorer API answered 403. **Inference:** the same deployer, salt and initcode give
the same address, so the address and the matching runtime are the identity that matters.

**What the chain's operators can still do.** The contract has no admin, but the chain has: the sequencer
can exclude transactions, and the chain is upgradable. See the
[earlier note's §6](robinhood-chain.md#6-permissioning-and-access).

### 1.4 What live traffic does and does not show

```bash
cast logs --address $X --from-block $((head-250000)) --to-block $head --rpc-url $M --json
```

95 logs in blocks 104093767–104286666: 19 each of `ChannelCreated`, `Deposited`, `Claimed`, `Refunded`
and `Settled`. All 19 configs name USDG with `withdrawDelay` 900, and all name one receiver
(`0xd4ab…a733`) and one `receiverAuthorizer` (`0x085f…1995`), the contract the claim and refund transactions
sampled were sent to. Testnet returned 0
logs over the last 100,000 blocks. CONFIRMED-CODE.

**This traffic does not exercise the connector's path, in two ways:**

- **No EIP-3009 deposit.** All 19 deposit receipts hold one USDG `Transfer`, and none has
  `AuthorizationUsed` (`0x98de5035`), any log naming the ERC-3009 collector, or a log from Permit2.
  How those deposits collect funds was not identified.
- **No timed withdrawal.** There are no `WithdrawInitiated` or `WithdrawFinalized` logs. Each channel
  ended by `Refunded`, while the connector's exit is `initiateWithdraw` then `finalizeWithdraw`.

So §3.3's fork run is the evidence for those two steps, not these channels.

---

## 2. The code path, from table to backend

Read at the pin. CONFIRMED-CODE.

| Step            | Where                                                                           | What happens                                                                                       |
| --------------- | ------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------- |
| Parse           | `crates/connector-config/src/settlement.rs:63`, `:110`                          | `eip155` is a map from any string to a typed table, `deny_unknown_fields`                          |
| Key to chain id | `settlement.rs:1219`, `crates/connector-domain/src/network.rs:218`              | Decimal digits, no leading zero, fits `u64`. **No list.** 4663 is `Network::Eip155(4663)`          |
| Resolve         | `settlement.rs:976`                                                             | Checks URL shape, address shape, non-zero decimals, key location, required domain keys             |
| Transport       | `crates/connector-cli/src/runtime.rs:1910`, `:1949`                             | One `RpcTransport` per table, from `rpc_url`                                                       |
| Chain id check  | `runtime.rs:604`, `:659`                                                        | `eth_chainId` must equal the key, or `EvmChainIdMismatch`                                          |
| Bind            | `runtime.rs:651`, `crates/connector-settlement-evm/src/batch_settlement.rs:166` | See §3.1                                                                                           |
| Deposit terms   | `runtime.rs:700`                                                                | See §3.2                                                                                           |
| Publish         | `runtime.rs:1246`, `:1263`                                                      | `eip155:<id>` from the chain, name and version from config, into the greeting's `batchSettlements` |
| Watch           | `runtime.rs:2016`                                                               | One `EvmBatchWatcher` per table                                                                    |

**Is any chain id allowlisted, hardcoded or mapped? No.** A scan of every non-test source under
`crates/*/src`, for known chain ids and for `chain_id ==`, `match … chain_id`, allowlist names,
`is_testnet`, chain names and block-time constants, found these and nothing that gates a network:

- `crates/connector-cli/src/init.rs:50`, `DEVNET_EVM_CHAIN_ID = 84532`: what `connector init` writes into
  a scaffold, not a check.
- `crates/connector-config/src/error.rs:485`, `:872-890`, and `crates/connector-operator/src/lib.rs:748`:
  example chain ids in refusal text.
- `crates/connector-cli/src/batch_settlement.rs:91` and
  `crates/connector-client-edge/src/batch_settlement.rs:113`: look up a domain among **this node's own
  tables**, so 4663 matches once its table exists.
- `crates/connector-domain/src/network.rs:110`, `is_mainnet`: XRPL only.

The other `allowed`/allowlist hits are URL schemes and operator write keys.

**x402 scheme terms** carry the network as `eip155:<id>` with no chain-specific branch
(`runtime.rs:1263-1277`). **ADR 0085's** rule admits x402's own code, and this is it.

---

## 3. Would each check pass, and does anything break?

### 3.1 How boot verifies the contract

`EvmBatchSettlementBackend::connect` makes three checks, in order:

1. **Code present.** `eth_getCode` is merely non-empty (`batch_settlement.rs:178-185`).
   **There is no code-hash comparison**, so the immutables cannot trip it. The committed and live keccaks
   do differ (`0xbaeca2aa…` committed, `0xe7c81673…` on 4663, `0x1da8b66d…` on 46630), and it does not matter.
2. **`decimals()`** equals the table's value (`:186-199`). USDG answers 6.
3. **`getChannelId(probe)`** equals what this node computes under `{chain id from the RPC, X}`
   (`:202-232`). This is the real identity check, and it is why the chain id immutable is harmless: it
   only has to agree with the chain.

### 3.2 Does anything assume USDC?

**No.** CONFIRMED-CODE, except where a run is named.

| Assumption checked               | Finding                                                                                                                                                                            | Citation                                                                                              |
| -------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| Token address                    | Config, one per table                                                                                                                                                              | `settlement.rs:112`, `:1010`                                                                          |
| EIP-712 name and version         | Config, required, no default. Compared with the token's `DOMAIN_SEPARATOR()` by hash, so USDG's reverting `version()` is never called                                              | `settlement.rs:1023-1028`, `batch_payer.rs:271-304`                                                   |
| Domain live                      | `Global Dollar` / `1` hashes to `0x7a3d7400…2036` on 4663 and `0xb1debe91…950b` on 46630, each equal to the live value. **Testnet's version was an open item** in the earlier note | `cast keccak`, `cast call … DOMAIN_SEPARATOR()`                                                       |
| Decimals                         | Asserted, never applied: amounts are base units                                                                                                                                    | `batch_settlement.rs:194`, `crates/connector-settlement-evm/src/bindings.rs:10-13`                    |
| Collector and signature function | `authorizationState` answering selects `ERC3009DepositCollector`; a revert selects Permit2. USDG answers `false` on both networks, so ERC-3009                                     | `batch_payer.rs:234-259`                                                                              |
| What's called on the token       | Only `decimals`, `authorizationState`, `DOMAIN_SEPARATOR`, `allowance`, `approve`, and the contract's own transfers. USDG lacks none of the ones the ERC-3009 route uses           | `bindings.rs:14-19`, `:45-52`                                                                         |
| Signature shape                  | The collector takes `bytes` signature data, and USDG accepted the connector's 65-byte signature                                                                                    | CONFIRMED-RUN, §3.3                                                                                   |
| `[[tokens]]`                     | Optional unless the node has an XRPL table or deals. If declared, the row must match                                                                                               | `crates/connector-config/src/peering_asset.rs:20-29`, `crates/connector-config/src/config.rs:561-572` |

The word `USDC` appears in non-test source only in refusal text and doc comments.

### 3.3 Tested: the real binary

`connector` was built at `08dbc279` with `cargo build -p connector --bin connector`, after the `rustc`
wait. Keys were throwaway files in the scratchpad, none in this repository. The settlement key held 0 ETH
on mainnet (`cast balance`), so it could not pay for anything.

**Boots against the public RPCs** (minimal table, default terms; `timeout` killed it, so exit 124 means
still serving). CONFIRMED-RUN:

| Config                                                           | Exit | Printed               |
| ---------------------------------------------------------------- | ---- | --------------------- |
| `[settlement.eip155.4663]`, USDG                                 | 124  | `connector listening` |
| `[settlement.eip155.46630]`, testnet USDG                        | 124  | `connector listening` |
| 4663 plus `[[tokens]]` `eip155:4663/erc20:0x5fc5…d168`           | 124  | `connector listening` |
| 4663 plus `[settlement.eip155.84532]` (Base Sepolia devnet USDC) | 124  | `connector listening` |

**Refusals from the same chain, showing the checks are live.** CONFIRMED-RUN:

| Change                   | Exit | Printed, abridged                                                                                       |
| ------------------------ | ---- | ------------------------------------------------------------------------------------------------------- |
| `USDC` / `2`             | 1    | `asset_eip712_name = "USDC" and asset_eip712_version = "2" are not the EIP-712 domain of token 0x5fc5…` |
| `decimals = 18`          | 1    | `decimals is 18, but token 0x5fc5… reports decimals() = 6`                                              |
| Keyed `8453`, 4663's RPC | 1    | `[settlement.eip155.8453] is keyed for chain id 8453 … but its rpc_url answers eth_chainId with 4663`   |
| Token `0x…0001`          | 1    | `could not be bound … failed to decode empty bytes`                                                     |

**Live, with sends refused.** A small local JSON-RPC proxy forwarded to `M`, logged every method and
refused `eth_sendRawTransaction`. The node ran at `min_withdraw_delay_secs = 900`, for 10 s watching.

- **36 s idle:** `eth_chainId` 2, `eth_getCode` 1, `eth_call` 5, `eth_blockNumber` 4, `eth_getLogs` 3.
  The logs queries were scoped to `X` and `WithdrawInitiated`, each about 100 blocks.
- **Greeting** (`GET /ilp`): `batchSettlements[0]` is `network: eip155:4663`, the USDG address,
  `withdrawDelay: 900`, `name: Global Dollar`, `version: 1`, `assetTransferMethod: eip3009`.
- **`POST /channels`** on a USDG offer, driving the real send path: `eth_getTransactionCount(pending)` =
  `0x0`, `eth_getBlockByNumber`, `eth_feeHistory`, then `eth_estimateGas`, which Nitro answered
  `insufficient funds for transfer`. The operator saw
  `batch-settlement backend error: (code: -32000, message: insufficient funds for transfer, data: None)`.
  `eth_sendRawTransaction` count: **0**.

**Full lifecycle on a local fork of mainnet.** `anvil --fork-url $M --port 8599`, with balances set by
`anvil_setBalance` and a USDG holder impersonated to fund both nodes. Every write stayed on the local
fork, and anvil does not forward them upstream. Two unmodified nodes, following `local/two-hop`'s shape,
each with the §6 table pointed at the fork. CONFIRMED-RUN:

| Step             | Drive                                                         | Result                                                                                                                            |
| ---------------- | ------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| Open, both ends  | `POST /peers` with `network: eip155:4663`, deposit 1,000,000  | `status: created`, then `open`, collateral 1,000,000                                                                              |
| Top-up           | `POST /channels/:id/fund`, 500,000                            | Collateral 1,500,000                                                                                                              |
| Paid packets     | `connector send --expect-fulfill`, twice, amount 1000         | `FULFILL`, exit 0. B's journal: two `inbound_claim_accepted`, cumulative 1000 and 2000                                            |
| Claim and settle | Restart B, whose sweep ticks on start                         | `batch-settlement sweep claimed and settled`, `channels:1`, `settled:"2000"`; `channels(id)` shows 2000 claimed; B's USDG up 2000 |
| Withdraw         | `POST /channels/:id/withdraw`, `evm_increaseTime 1000`, again | `step: started`, then `step: finished`; A's USDG up by the 1,498,000 left                                                         |

Balances conserve. A started at 200,000,000 base units, deposited 1,500,000 and recovered 1,498,000, so it
ends at 199,998,000: down exactly the 2,000 B earned. B ends at 199,002,000, with 1,000,000 still in its own
channel toward A. Fork-local figures only.

**The ERC-3009 route on real USDG code.** A separate fork run opened one channel and read the receipt: a
type 2 transaction to `X`, selector `0x140f1e75`, status 1, gas 143,922, with these logs in order:

```text
USDG  AuthorizationUsed  from A
USDG  Transfer           A -> 0x4020…0004 (ERC3009DepositCollector)
USDG  Transfer           0x4020…0004 -> X
X     ChannelCreated
X     Deposited
```

This settles the earlier note's unread facet: USDG's closed-source EIP-3009 facet accepts exactly what
the connector signs. Fees were `maxFee` 36,409,502 and tip 1,000,000 wei from `market_fees`.

**What the fork does not prove.** Anvil runs its own EVM with no Arbitrum precompiles
(`cast call 0x…64 'arbBlockNumber()'` gives `InvalidFEOpcode`), so USDG and x402 reach no precompile on
this path. **Inference**, from their success there. Nitro's RPC shapes, fees and sequencer are §3.4's
subject, not the fork's.

### 3.4 Nitro behaviour

| Concern                               | Finding                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               | Tag                                                                                       |
| ------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------- |
| **Block and receipt decoding**        | A scratch program on ethers 2.0.14 decoded live `get_block` (extras `l1BlockNumber`, `sendCount`, `sendRoot` land in `other`), `get_transaction`, `get_transaction_receipt` (extras `gasUsedForL1`, `l1BlockNumber`) and a full block including type 106 (`0x6a`, Arbitrum internal) transactions. **The earlier note's top friction does not exist**                                                                                                                                                                                                 | CONFIRMED-CODE                                                                            |
| **Fee model**                         | Writes fill EIP-1559 fees from the market (`crates/connector-settlement-evm/src/send.rs:290`). Live `baseFeePerGas` about 0.02 gwei, and `eth_feeHistory` rewards all `0x0`, so the tip is the floor of 0.001 gwei and the cap about 0.04 gwei (`send.rs:76`, `:83-92`). The cap outruns the base fee unless it doubles between read and send. ethers' 3 gwei default is not used                                                                                                                                                                     | CONFIRMED-RUN. Whether Nitro charges that tip: UNVERIFIED, at most about 5% of a tiny fee |
| **Gas with L1 data fees**             | `eth_estimateGas` covers both ([Robinhood](https://docs.robinhood.com/chain/gas-and-fees)), and the connector takes the estimate unpadded (ethers `fill_transaction`). Nitro itself pads: during estimation it uses 7/8 of the gas price, floored at the minimum base fee, and pads the L1 cost by 10% ([`tx_processor.go`](https://github.com/OffchainLabs/nitro/blob/ccc5c828d9ea22f161efc784bac4d4a6559b7e59/arbos/tx_processor.go), lines 32 and 577-601). An under-estimate would be reported as `reverted on chain`, not missed (`send.rs:423`) | CONFIRMED-CODE. Sufficiency in practice: UNVERIFIED                                       |
| **Costs**                             | Live, at about 0.0202 gwei: a `settle` used 60,035 gas with `gasUsedForL1` 0; a third party's claim 67,297; a refund 105,834; a deposit through a wallet 250,954 with L1 1,397. `NodeInterface.gasEstimateL1Component` on 420 bytes gave 1,089 L1 gas units                                                                                                                                                                                                                                                                                           | CONFIRMED-CODE                                                                            |
| **0.1 s blocks**                      | 13 sampled blocks shared two timestamps. Nothing here assumes a block rate: `confirm` waits on wall-clock deadlines of 30 s unseen and 180 s total (`send.rs:124-133`). The comment "90 Base blocks" at `send.rs:119` is only a description                                                                                                                                                                                                                                                                                                           | CONFIRMED-CODE                                                                            |
| **Log windows**                       | `MAX_BLOCK_RANGE` 2,000 (`crates/connector-settlement-evm/src/batch_watch.rs:72`) is about 200 s here. The watch reads cursor to head in 2,000-block chunks (`batch_watch.rs:146-168`), every 10 s to 15 min by the delay (`crates/connector-settlement/src/watch_interval.rs:29`). So the default one-day delay takes about 5 queries per 15-minute pass; the floor takes one per 10 s. **It scales.** The public RPC served 100,000 and 250,000-block ranges                                                                                        | CONFIRMED-RUN, CONFIRMED-CODE                                                             |
| **Cursor on failure**                 | The cursor moves only after success (`batch_watch.rs:424`), so catching up after an outage costs about 18 queries per hour of backlog. A restart starts at head, and the sweep covers what a watch missed (`batch_watch.rs:389-391`)                                                                                                                                                                                                                                                                                                                  | CONFIRMED-CODE                                                                            |
| **`eth_getLogs` provider limits**     | Queries are address-scoped (`crates/connector-settlement-evm/src/log_query.rs:40`). A paid provider's limits were not tested                                                                                                                                                                                                                                                                                                                                                                                                                          | UNVERIFIED                                                                                |
| **Finality**                          | A receipt is confirmation (`send.rs:423-429`), the sequencer's soft confirmation here. **Inference:** the same shape as any single-sequencer L2 the connector settles on, and not new. `safe` and `finalized` are never read                                                                                                                                                                                                                                                                                                                          | CONFIRMED-CODE                                                                            |
| **Timestamps**                        | Whole seconds in `timestamp` and `blockTimestamp` over RPC; ties are normal. x402 uses `block.timestamp` only for withdrawal delays and never `block.number` (source lines 340-365), and `chain_now` compares seconds (`batch_payer.rs:649`). **No sub-second value reached this path**                                                                                                                                                                                                                                                               | CONFIRMED-CODE                                                                            |
| **Reads pinned to a receipt's block** | `snapshot_after` needs recent state only (`batch_settlement.rs:317`), and retries for 6.2 s. Public RPC is not an archive, per the earlier note; not re-tested                                                                                                                                                                                                                                                                                                                                                                                        | Inference                                                                                 |
| **Polling and rate limits**           | `confirm` polls at 100 ms when direct (`send.rs:124-133`). 60 `eth_blockNumber` calls at concurrency 10 all answered 200, so the limit is above that burst. Robinhood calls the public endpoints not for production, per the earlier note                                                                                                                                                                                                                                                                                                             | CONFIRMED-CODE. Real limit: UNVERIFIED                                                    |
| **No public mempool**                 | **Inference:** `pending` is about `latest`. `Sender` counts nonces locally and reseeds only after a refusal or a lost answer. A re-broadcast of an already-sequenced transaction may answer `nonce too low` rather than `already known`, but `resolve_lost_send` looks the hash up first, and an unresolved send goes back to `confirm` by hash with its bytes kept, never signed anew (`send.rs:373-381`, `:354-365`). So the failure mode is a wait, not a double spend                                                                             | Nitro's exact wording: UNVERIFIED                                                         |
| **Sequencer exclusion**               | A transaction tied to a sanctioned address is excluded ([Robinhood](https://docs.robinhood.com/chain/differences-from-ethereum)). **Inference:** it would surface as `send.rs`'s `not observed`, which already refuses to call that proof of a drop                                                                                                                                                                                                                                                                                                   | UNVERIFIED                                                                                |

---

## 4. What was not tested

- **A transaction through the live sequencer**, on either network. Sending was out of scope. Everything up
  to `eth_sendRawTransaction`, and decoding of the receipt and transaction that follow, was exercised
  separately, but not as one path.
- **The shipped image** (`make local-verify` runs on its own chains, not this one). Only the debug binary ran.
- **A real second operator.** The fork run's two nodes shared a machine.
- **`toon-client` paying a node on 4663.** See §5.
- **A facilitator deposit** (ADR 0076). `api.solvador.com/supported` and `x402.dexter.cash/supported` both
  list `batch-settlement` on `eip155:4663`, and neither lists 46630. Whether either relays a deposit toward
  a channel this node admits is unverified.
- **Testnet lifecycle.** Testnet USDG's availability to a throwaway key was not examined.
- **USDG's freeze or pause during a channel.** Neither `X` nor the ERC-3009 collector is frozen today, and
  `paused()` is false on both networks.

---

## 5. The other side of a peering

A peering is two channels (ADR 0075), so each operator needs, briefly:

- A `[settlement.eip155.4663]` table on the **same token**. Otherwise `TokenNotShared` on opening
  (`batch_payer.rs:318`), or `TokenNotSettled` on admission (`batch_settlement.rs:443`).
- ETH for gas and USDG for collateral, on its own settlement key. A channel's opening deposit comes from
  its own payer.
- `[node] http_endpoint`, `[operator]` keys, and `POST /peers` naming the other's URL. `network` may be
  left out with one shared network (`crates/connector-operator/src/lib.rs:710-712`). `local/`'s
  `peer_allow_plaintext_endpoints` was needed only because the fork run used `http://`.
- **To reach USDC elsewhere**, a dealing boundary with `[[tokens]]` and a declared rate (ADR 0071). Config
  only as far as the connector goes, and not run here.

**A client on `toon-client`.** Its offer parser takes any `eip155:` network, with name and version from
the offer ([`offers.ts`](https://github.com/toon-protocol/toon-client/blob/6cfd3471e7e23c65f3a96a3790a4779af341ae81/packages/client/src/channel/batch-settlement/offers.ts),
line 95), and its chain id comes from a generic regex
([`evm.ts`](https://github.com/toon-protocol/toon-client/blob/6cfd3471e7e23c65f3a96a3790a4779af341ae81/packages/client/src/channel/batch-settlement/evm.ts),
line 224). Its presets name only 84532. **Inference:** no allowlist there either. Not run, and `rig` and
`swap` were not read.

---

## 6. Minimal config

**Tested for boot, exactly this shape, on 2026-11-02** (`connector <file>` reached `connector listening`
against the public RPC). Paths are placeholders. Not tested with a funded key on the live chain.

```toml
client_edge_addr = "127.0.0.1:0"
state_dir = "/path/to/state"

[signer]
key_file = "/path/to/signer.key"

[settlement.eip155.4663]
rpc_url = "https://rpc.mainnet.chain.robinhood.com"   # rate-limited; use a provider for real traffic
token_address = "0x5fc5360D0400a0Fd4f2af552ADD042D716F1d168"
decimals = 6
asset_eip712_name = "Global Dollar"
asset_eip712_version = "1"

[settlement.eip155.4663.key]
key_file = "/path/to/settlement.key"
```

Testnet is the same with `46630`, `https://rpc.testnet.chain.robinhood.com` and token
`0x7E955252E15c84f5768B83c41a71F9eba181802F`, also tested for boot.

**Required** in the table: `rpc_url`, `token_address`, `decimals`, `key`, `asset_eip712_name`,
`asset_eip712_version` (`settlement.rs:111-114`, `:1023-1028`). **Optional:** `min_withdraw_delay_secs`
(default 86,400, floor 900, ceiling 30 days), `asset_transfer_method` (default `eip3009`, which is right for
USDG), `facilitator_url`, `rpc_via_socks_proxy`.

**To peer, add** (the shape the fork run used, untested on the live chain):

```toml
[node]
addresses = ["g.your.prefix"]
http_endpoint = "https://your.host/ilp"

[operator]
bearer_token_file = "/path/to/operator-bearer-token"
write_keys_file = "/path/to/operator-write-keys"
```

---

## 7. Open questions

For the owner:

1. **Authorise one testnet send?** A funded key on 46630, a real open, voucher, claim and withdrawal. It is
   the one step left, and it should stay off the gate (ADR 0084 decision 8).
2. **Is a USDG leg wanted at all?** Earnings are routing fees over peerings, so it pays only if counterparties
   hold USDG on 4663. Also the [earlier note's](robinhood-chain.md#open-questions) trust question.
3. **Does ADR 0084 decision 7's "program" include the token?** The fork showed USDG behaves, but a gate test
   would have to place its proxy, facets and storage.

Could not verify, or did not:

- Who deployed on mainnet, and when: the explorer API answers 403 and the public RPC is not an archive.
- USDG's EIP-3009 facet source. Its behaviour was run on a fork, not read.
- How the 19 live deposits collect funds, and who runs them.
- Provider `eth_getLogs` limits, the public RPC's real rate limit, and Nitro's exact refusal wording.
- Whether Nitro charges the 0.001 gwei tip.

### Where the earlier note was wrong or is now out of date

- **"Probably config-only" and "no connector was built or booted".** Now run: it boots on both networks, and
  the lifecycle passes on a fork.
- **USDG's EIP-3009 behaviour "inferred from … the 18 channels in §4".** That inference does not hold. None of
  the 19 deposits now in the window used EIP-3009: no `AuthorizationUsed`, no ERC-3009 collector. The fork
  run is what shows it works.
- **"18 USDG channels went through their full lifecycle".** Those events are real but they end in `Refunded`.
  There is no `WithdrawInitiated` or `WithdrawFinalized`, so they are not the connector's exit path. They also
  all name one receiver.
- **"Offsets 16009 to 16097" for the `x402BatchSettlement` difference** are hex-string positions. In bytes the
  difference is two ranges, 8004–8006 and 8017–8048. The conclusion stands.
- **"Client decoding" friction.** Did not materialise.
- **"Whether the watcher's chunking scales … was not traced".** Traced: it does.
- **"Testnet's domain version was not checked".** Now checked: `Global Dollar` / `1`.

Confirmed unchanged: both deployments, byte-identical collectors, Permit2's immutables-only difference,
`SignatureChecker` absent and harmless, `version()` reverting, and x402's README having no Robinhood row.
PRs [#3770](https://github.com/x402-foundation/x402/pull/3770) and
[#3761](https://github.com/x402-foundation/x402/pull/3761) are still open. ADR 0086's header still says
"not yet built" while `settlement.rs` parses these tables and two tables booted together here, so that header
is out of date.
