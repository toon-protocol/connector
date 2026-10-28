# Chains the harness spawns: what a shared tier would cost, chain by chain

> **Research note, not a decision record.** It decides nothing. The question is issue
> [#1568](https://github.com/toon-protocol/connector/issues/1568) ("Chains that can't be spawned per
> test", part of map [#1563](https://github.com/toon-protocol/connector/issues/1563); Mina sibling
> [#1554](https://github.com/toon-protocol/connector/issues/1554)). The owner settled its shape on
> 2026-10-28:
>
> - one general rule, to be recorded as an ADR amending CLAUDE.md's "Tier 3 does not use the
>   Docker containers";
> - a per-test chain stays the default;
> - a chain that costs too much to spawn per test may use a **shared tier**: one chain per test
>   binary, spawned lazily by the harness itself (e.g. behind a `OnceLock`), failing CI loudly when
>   its binary or image is missing, and never a hand-started container. Tests get isolation from
>   fresh funded accounts and fresh channels. A test that moves the clock, or needs chain-global
>   state, still gets its own chain;
> - a public testnet never counts toward the gate;
> - the admission condition is that the harness can spawn a chain that runs the real on-chain
>   logic. Stubs and emulations (ADR 0007) do not count.
>
> This note gathers the facts the next round needs to turn that into numbers. It builds on
> [`x402-pending-chain-bindings.md`](x402-pending-chain-bindings.md) (cited as **BINDINGS**, chiefly
> §2.5, §3.5, §5.5 and D6–D8) and [`mina-x402-batch-settlement.md`](mina-x402-batch-settlement.md)
> (cited as **MINA-NOTE**, §6 and D11), and does not redo their ground. Primary sources only.
> Written 2026-10-28, and every source was read or measured that day.

**Evidence tags.**

- **CONFIRMED-CODE**: read in source at a pinned commit, or read from this repository's CI logs.
- **CONFIRMED-DOCS**: read in the maintainer's own documents, Docker Hub pages, release notes or
  workflow files.
- **MEASURED**: run on the author's workstation (16 cores, 31 GB, Linux) on 2026-10-28, one or two
  runs each. These are rough figures, not benchmarks.
- **UNCONFIRMED**: none of the above, with the reason given.
- **Inference**: this note's reasoning from confirmed facts, not a fact.

**Sources pinned.**

- **This repository**: `HEAD` `9a039468` (branch `adr-0077-zk-note`). CI evidence comes from the
  green `main` run [40701157357](https://github.com/toon-protocol/connector/actions/runs/40701157357)
  at `79561fc7`, job "Rust Workspace Gate" (`122861068859`), cited as **RUN**.
- **xrpld**: [`XRPLF/rippled@05847a3c`](https://github.com/XRPLF/rippled/tree/05847a3c908a9b3eaf43ddbd9af3c81214e60268)
  (cited as **R**), image `xrplf/xrpld:3.4.1`.
- **Mina**: [`MinaProtocol/mina@f92f3199`](https://github.com/MinaProtocol/mina/tree/f92f31995c252052f932b4b7f25d46c9b1331943)
  (**M**), [`o1-labs/zkapp-cli@421cf378`](https://github.com/o1-labs/zkapp-cli/tree/421cf3781f441e159f1c87f2688fc598193bb966)
  (**Z**), image `o1labs/mina-local-network:compatible-latest-lightnet`.
- **Cardano**: [`bloxbean/yaci-devkit@v0.12.0`](https://github.com/bloxbean/yaci-devkit/tree/v0.12.0)
  (**YACI**), [`bloxbean/yano@7f025461`](https://github.com/bloxbean/yano/tree/7f02546192e136bf154da22c92d1adea5563a35e)
  (**YANO**), [`txpipe/dolos@v1.6.2`](https://github.com/txpipe/dolos/tree/v1.6.2) (**DOLOS**),
  [`IntersectMBO/cardano-node@11.2.0`](https://github.com/IntersectMBO/cardano-node/tree/11.2.0).
- **Hedera**: [`hiero-ledger/solo@v0.95.0`](https://github.com/hiero-ledger/solo/tree/v0.95.0)
  (**SOLO**), [`hiero-ledger/hiero-consensus-node@v0.78.1`](https://github.com/hiero-ledger/hiero-consensus-node/tree/v0.78.1).
- **GitHub runners**: [GitHub-hosted runners reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners),
  [larger runners](https://docs.github.com/en/actions/reference/runners/larger-runners),
  [`actions/runner-images@aa2d9f9` Ubuntu 24.04 readme](https://github.com/actions/runner-images/blob/aa2d9f91221232debee20fcb34746384a3688579/images/ubuntu/Ubuntu2404-Readme.md)
  (image 20261023.334.3).

---

## Verdict

- **Today the gate spawns one chain per test, and Solana's validator is already the costly one.**
  - Anvil is ready in about 0.03 s at about 95 MB RSS.
  - `solana-test-validator` answers RPC in 0.2 s, but produces its first slot only after about
    13.6 s locally (v3.1.12), at about 380 MB. On CI's pinned v2.1.21, a one-test binary that
    spawns a validator finishes in 4.7 s.
  - Solana-bound binaries are about 64 % of the 437 s that test binaries ran in RUN (§1).
- **Nothing in the repository shares a chain yet.** There is no `OnceLock` around a chain anywhere
  in `crates/`. There is one precedent for isolating tests by fresh channels on one chain: the
  receiving contract suite already runs every case against one chain with a fresh channel per case
  (§1.4).
- **Rust never drops a `static`** ([Rust reference](https://doc.rust-lang.org/reference/items/static-items.html)).
  So a `OnceLock<Chain>` leaks its child process or container when the test binary exits, unless the
  harness arranges otherwise. That is the shared tier's main engineering problem (§6).
- **The four candidates**, against the owner's tiers:
  - **XRPL** (`xrpld` stand-alone) fits **per test**: ready in 1–3 s and 72–322 MB.
  - **Mina lightnet** fits the **shared tier only**: SYNCED in 37–46 s, 0.6–0.7 GB, and about 26 s to
    fund a new account. Its default image **cannot** run real proofs: the `-devnet` image is needed
    for `PROOF_LEVEL=full`, and that image's costs were not measured.
  - **Cardano** has two shapes. **Dolos's emulator** is a 22 MB Rust binary that produces its own
    blocks and runs phase-2 Plutus through pallas/amaru-uplc. **Yaci DevKit** is heavier (a 931 MB
    image, or a 65 MB native CLI). Neither has a published startup or RAM figure. Whether either one
    "runs the real on-chain logic" under Q4 is itself a question, because neither runs the Haskell
    ledger.
  - **Hedera** (Solo) fails even the shared tier as written. It needs Kubernetes (kind), at least
    12 GB, takes 3–5 min warm, and is not spawned by a harness in any first-party example. The only
    real-logic in-process option is a JUnit harness inside the consensus node's own Java tests.
- **Standard runners are where this repository's CI runs, and they cap what any tier can do.** The
  repository is public, so `ubuntu-latest` gives 4 CPU, 16 GB and 14 GB SSD for free. Larger runners
  are always billed, even for public repositories, and need a Team or Enterprise plan (§5).

---

## 1. Baseline: what the gate spawns today

### 1.1 How the two harnesses spawn

- **`Anvil::spawn(base_port)`**:
  - it forks `anvil --accounts 2 --balance 10000` on a port built as
    `base_port + pid % 1000 + atomic counter`;
  - it polls `eth_chainId` every 50 ms, up to 200 times (10 s);
  - `Drop` kills the child.
  - CONFIRMED-CODE: `crates/connector-settlement-evm/src/test_support.rs:136-202`.
- **The gate check.** `require_anvil()` panics when `CI` is set and skips with a message otherwise
  (`test_support.rs:58-74`). Its Solana twin is `require_solana_test_validator()`
  (`crates/connector-settlement-solana/src/test_support.rs:388-407`). CONFIRMED-CODE.
- **`SolanaValidator::spawn`**:
  - it forks `solana-test-validator --reset --quiet` with its own temp ledger;
  - the RPC port is `19900 + pid % 500 + 50 × counter`, the faucet port is RPC + 1, and the dynamic
    port range is RPC + 2 to RPC + 40;
  - it loads `payment-channels` and p-token with `--bpf-program`, polls `getVersion` every 100 ms up
    to 600 times (60 s), then checks that p-token is served at the SPL Token id;
  - `Drop` kills the child, and dropping the `TempDir` deletes the ledger.
  - CONFIRMED-CODE: `test_support.rs:409-517`.
- **How the port scheme behaves (Inference).**
  - Collisions are unlikely rather than impossible: two binaries whose pids agree mod 500 or 1000
    collide, and so would the 51st validator in one binary.
  - Neither harness asks the OS for a free port. alloy's Anvil binding passes `-p 0` and parses the
    real port from stdout instead
    ([`alloy@419c670` `anvil.rs` L419-485](https://github.com/alloy-rs/alloy/blob/419c670076a27091a08fe51f8e96394d71e56bc7/crates/node-bindings/src/nodes/anvil.rs#L419-L485)).
    CONFIRMED-CODE.
- **Clocks today.**
  - EVM tests move time with `evm_increaseTime` + `evm_mine` (`test_support/x402.rs:403-417`) or
    `evm_setNextBlockTimestamp` (`connector-rate-source-evm/tests/support/mod.rs:383`).
  - The only Solana clock lever used is `--warp-slot 10000` at spawn
    (`connector-settlement-solana/tests/batch_watch.rs:96`).
  - The paying contract suite's `let_delay_pass` documents that "on a validator, whose clock cannot
    be moved, this may do nothing" (`connector-settlement/src/batch/contract.rs:398-404`).
  - CONFIRMED-CODE.

### 1.2 Startup and memory

| Chain                                        | RPC answers | Usable (first slot / first tx confirmed)                                             | RSS                  | Source                                                     |
| -------------------------------------------- | ----------- | ------------------------------------------------------------------------------------ | -------------------- | ---------------------------------------------------------- |
| anvil 1.7.1                                  | 0.03 s      | 0.03 s (automine)                                                                    | ~95 MB               | MEASURED ×3                                                |
| solana-test-validator 3.1.12 + both fixtures | 0.23 s      | slot > 0 at 13.6 s; airdrop confirmed at 14.0 s                                      | ~380 MB, ledger 8 MB | MEASURED ×2                                                |
| solana-test-validator 2.1.21 (CI pin)        | —           | a one-test binary that spawns one validator and lands a voucher: **4.67 s** in total | —                    | CONFIRMED-CODE (RUN, `batch_settlement_solana_end_to_end`) |

- **The harness and readiness.** It returns once `getVersion` answers, which is before the first
  slot, so the first transaction in each test pays the rest of the wait. **Inference** from
  `test_support.rs:483-491` and the measurement.
- **Why measure locally at all.** The workstation runs v3.1.12, not the gate's v2.1.21 (CLAUDE.md
  and `ci.yml` pin 2.1.21 because v3 needs io_uring). The v3 figure is an upper bound for the gate,
  not the gate's own number. MEASURED, CONFIRMED-CODE.

### 1.3 How many chains one `cargo test --workspace` spawns

- **Call sites.** The tree has 55 `Anvil::spawn*` call sites outside the harness, in 21 files, and
  22 `SolanaValidator::spawn*` call sites, in 16 files. Many sit in a per-test `world()` helper, so
  the number of runtime spawns is higher than the number of call sites. CONFIRMED-CODE (grep at
  `HEAD`).
- **Per-file examples.**
  - `lagging_reads.rs` has 15 tests, and each calls `world_admitting()`, which spawns a validator
    (`connector-settlement-solana/tests/lagging_reads.rs:241-242`).
  - `connect_identity.rs` has 7 tests with 3 spawn sites.
  - CONFIRMED-CODE.
- **Estimate (Inference).** The gate spawns on the order of **45–55 Solana validators and roughly
  100 anvils**. This counts test functions in files whose `world()`/helper spawns. It is not exact:
  the static count timed out on transitive helpers, and running `--workspace` to count was out of
  scope.
- **Concurrency.** Cargo ran RUN's 116 test targets one after another. Inside a target, tests run in
  parallel threads, so `lagging_reads` ran up to 15 validators at once on a 4-CPU runner and took
  **78.8 s**, the slowest target in the run. CONFIRMED-CODE (RUN log order and timings).

### 1.4 What RUN's timings say

- **Step durations.**

  | Step                                                  | Duration  |
  | ----------------------------------------------------- | --------- |
  | Job total                                             | 17 m 24 s |
  | Install Solana CLI                                    | 30 s      |
  | Install Foundry                                       | 3 s       |
  | `cargo build`                                         | 2 m 39 s  |
  | `cargo test` (includes a 2 m 27 s test-profile build) | 9 m 50 s  |
  | Clippy                                                | 1 m 36 s  |

  CONFIRMED-CODE (RUN step timings).

- **Where the test time goes.** Test targets ran for 436.7 s in total. **278.5 s (64 %)** of that is
  in Solana-bound targets: `lagging_reads` 78.8, Solana `batch_watch` 36.9, `close_watch` 24.0,
  `solana_paying` 23.5, the solana crate's unit tests 19.8, and so on. The slowest EVM-bound target,
  `batch_settlement_paying`, is 17.2 s for 13 tests. CONFIRMED-CODE.
- **No `timeout-minutes`.** The gate job sets none, so GitHub's default of 360 minutes applies
  ([workflow syntax](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax)).
  CONFIRMED-CODE, DOCS.
- **A precedent for isolating by fresh channels.** `assert_upholds_the_contract` takes one fixture
  and drives every case through `open(ChannelTerms)` on the **same chain**, as the same payer, with a
  fresh channel per case (`connector-settlement/src/batch/contract.rs:44-130`). That is the shared
  tier's isolation model, already used inside one test. CONFIRMED-CODE.

### 1.5 The doctrine text says two different things

- **ADR 0007 says "per run".** Its section "The 'real chain' in that tier is a local, disposable
  one" says tests "spawn their own throwaway chain **per run**". The risks it gives for a shared
  chain are all about chains shared **across** runs and long-lived networks: a single funded key
  contending for nonces across concurrent CI runs, faucets, public RPC, and devnet halts
  (`docs/adr/0007-testing-doctrine-fakes-yes-mocks-no.md:42-62`). CONFIRMED-DOCS.
- **CLAUDE.md says "per test"** (`CLAUDE.md:71-72`), and the code does per test. CONFIRMED-DOCS,
  CODE.
- **Inference.** A chain shared within one test binary, with a fresh key per test, avoids each of
  ADR 0007's named risks: no key crosses runs, and no third party is involved. The amending ADR can
  cite 0007's own reasons instead of overturning them.

---

## 2. The candidates

BINDINGS §2.5, §3.5 and §5.5 and MINA-NOTE §6 already cover what each chain is and why. This section
adds only figures and isolation facts.

### 2.1 XRPL: `xrpld` stand-alone

- **Startup.**
  - MEASURED: `server_info` answered 1.0 s and 3.2 s after `docker run xrplf/xrpld:3.4.1 -a --start`
    (two runs), reporting `server_state: full` at ledger 2.
  - No first-party figure exists; the
    [stand-alone guide](https://xrpl.org/docs/infrastructure/testing-and-auditing/start-a-new-genesis-ledger-in-stand-alone-mode)
    is silent.
  - xrpl-rust's CI polls every 2 s up to 60 times
    ([`integration_test.yml` L98-120](https://github.com/sephynox/xrpl-rust/blob/2a5191b1442d2a77c9a89ae860f71b319ef693f2/.github/workflows/integration_test.yml#L98-L120)).
    CONFIRMED-CODE. This updates BINDINGS §2.5, which marked startup UNCONFIRMED.
- **RAM.**
  - MEASURED: about 72 MiB at start, and about 322 MiB after 21 `ledger_accept`s and a Payment.
  - Stand-alone defaults `node_size` to `tiny` (R `src/xrpld/core/detail/Config.cpp#L276`).
  - The docs' "16 GB for testing" is for networked nodes
    ([system requirements](https://xrpl.org/docs/infrastructure/installation/system-requirements)).
- **Image and packaging.**
  - The official image is `xrplf/xrpld` (106 MB compressed, amd64 only;
    [Docker Hub](https://hub.docker.com/r/xrplf/xrpld/tags)), and `rippleci/xrpld` is 95 MB.
  - An apt package exists for Ubuntu 20.04+ / Debian 11+ x86_64
    ([install guide](https://xrpl.org/docs/infrastructure/installation/install-rippled-on-ubuntu)).
  - Licence ISC. CONFIRMED-DOCS.
- **Several instances per host.**
  - Every port is set in the config file (example config: admin HTTP 5005, WS 6006, gRPC 50051,
    peer 2459; R `cfg/xrpld-example.cfg#L1564-L1645`).
  - So are the fixed paths `[node_db]`, `[database_path]` and `[debug_logfile]` (`#L1681-L1695`).
  - The image's shipped config binds admin to 127.0.0.1, so a container needs a mounted config
    (R `docs/Docker.md`).
  - **Inference:** a native spawn with a temp config and temp dir per instance runs any number at
    once, just like `SolanaValidator`.
  - CONFIRMED-CODE.
- **Account isolation.**
  - Funding is a Payment from the genesis account, then one `ledger_accept`. MEASURED: about 230 ms
    per account.
  - xrpl.js funds each test wallet with 400 XRP this way
    ([`utils.ts` L31-35](https://github.com/XRPLF/xrpl.js/blob/44872583ca6cb32b41be6d62da017213bbb84bc8/packages/xrpl/test/integration/utils.ts#L31-L35)).
  - Reserves: with no `[voting]` section the code defaults are a 10-drop fee, 10 XRP base reserve and
    2 XRP owner reserve (R `src/xrpld/core/Config.h#L58-L68`; MEASURED). The docs say 1 / 0.2, and
    xrpl.js's config sets 20 / 5. The harness should set `[voting]` explicitly. CONFIRMED-CODE.
- **State shared between tests on one node.**
  - **Ledger closing is global.** Any test's `ledger_accept` closes the ledger for every test, and
    the close time it pushes forward stays pushed.
  - The genesis account's sequence is one counter, so parallel funders race on it unless they are
    serialised. **Inference** from the account model; not measured.
  - Fee escalation in stand-alone mode starts only past 1000 transactions in a ledger
    (`cfg/xrpld-example.cfg#L635-L638`). CONFIRMED-CODE.
- **Clock.**
  - `ledger_accept` takes no time argument (R `src/xrpld/rpc/handlers/admin/server_control/LedgerAccept.cpp`).
  - Each close is at least the previous close + 1 s (R `include/xrpl/ledger/LedgerTiming.h#L146-L158`),
    so N accepts move close time at least N seconds ahead of wall clock. MEASURED: 20 accepts in
    0.9 s put it about 18 s ahead.
  - PayChannel expiry, `CancelAfter` and `SettleDelay` all compare the **parent ledger's** close time
    (R `src/libxrpl/ledger/helpers/PaymentChannelHelpers.cpp#L89-L90`, `PaymentChannelClaim.cpp#L121-L123`).
  - xrpl.js waits out times exactly this way
    ([`utils.ts` L374-402](https://github.com/XRPLF/xrpl.js/blob/44872583ca6cb32b41be6d62da017213bbb84bc8/packages/xrpl/test/integration/utils.ts#L374-L402)).
  - **This revises BINDINGS D7's "no clock warp found":** a warp exists. It costs about one
    `ledger_accept` per second, so a 901 s floor is about 900 accepts, roughly 40 s at the measured
    rate. That is an **Inference** from the 20-accept measurement. It moves the clock for everyone,
    so such a test needs its own chain.

### 2.2 Mina: lightnet (`o1labs/mina-local-network`)

- **Startup and RAM.**
  - Docker Hub says a single node takes "~1-2 minutes" and about 850–970 MB after the initial spike
    ([Docker Hub](https://hub.docker.com/r/o1labs/mina-local-network)).
  - The docs say 4.5 GB to start and 1.5–2 GB to run
    ([testing with lightnet](https://docs.minaprotocol.com/zkapps/testing-zkapps-lightnet)).
  - zkapp-cli refuses below 3.5 GB of Docker memory (Z `src/lib/lightnet.js#L1368-L1385`).
  - MEASURED (single node, `RUN_ARCHIVE_NODE=false`): SYNCED at **46 s and 37 s**, at
    **566–694 MiB**. CONFIRMED-DOCS.
- **Image size.** 903 MB amd64 for `compatible`/`develop`, and 1028 MB for `master`/`4.0.2` (Docker
  Hub tags). CONFIRMED-DOCS.
- **Real proofs need a different image.** This sharpens MINA-NOTE §6.
  - zkapp-cli's `--proof-level full` is passed through as `PROOF_LEVEL`
    (Z `src/lib/lightnet.js#L164-L170`).
  - The lightnet image is built with `COMPILED_PROOF_LEVEL=none`, and its entrypoint
    `/root/spinup-testnet.sh` (L111-117, read inside the image) **exits** with "This is a lightnet
    image (no proofs). Use the devnet image for PROOF_LEVEL=full."
  - Full proofs therefore mean `o1labs/mina-local-network:<branch>-latest-devnet`.
  - **The devnet image's startup, RAM and block time are UNCONFIRMED.** No first-party figure exists,
    and it was not pulled.
  - Where the entrypoint's source lives publicly is UNCONFIRMED; it was read from the image.
- **Several instances per host.**
  - Ports are fixed inside the container (GraphQL 3085, nginx 8080, accounts manager 8181, archive
    8282, Postgres 5432), but `docker run -p 127.0.0.1:X:8080` remaps them.
  - MEASURED: two lightnets ran side by side on remapped ports, and both reached SYNCED. This settles
    MINA-NOTE §6's UNCONFIRMED.
  - zkapp-cli itself pins the container name `mina-local-lightnet` and 1:1 ports
    (Z `src/lib/lightnet.js#L94`, `#L1400-L1404`), so a harness would use `docker run`, not
    zkapp-cli. CONFIRMED-CODE.
- **Account isolation.**
  - The accounts manager (`GET /acquire-account`, `PUT /release-account`) serves about 1000
    pre-funded accounts (MEASURED: 1007 in `daemon.json`) at 1550 MINA each. MEASURED: an acquire
    took about 1.06 s.
  - A brand-new key can be funded from an acquired account at a 1 MINA creation fee
    (M `src/lib/node_config/profiled/lightnet.ml`). MEASURED: **26 s** to inclusion.
  - **Inference:** with about 1000 accounts, acquire/release gives each test its own funded account
    without any funding transaction, and that is lightnet's documented purpose.
- **Clock.**
  - Slots default to 20 s and can be changed with `SLOT_TIME`
    (M `scripts/mina-local-network/README.md`). Blocks come "every ~20-40 seconds"; k = 30.
  - Genesis is the container's start time (`-u delay_sec:0`).
  - No way to advance the global slot was found in the docs, zkapp-cli or the entrypoint.
    UNCONFIRMED (searched, not found).
- **State shared between tests.** The mempool (`pool_max_size = 3000`), slot and height are shared,
  and the account pool is shared too unless accounts are released. CONFIRMED-DOCS.
- **Licence.** The mina repository and zkapp-cli are Apache-2.0. The image itself states no licence
  and bundles Postgres and nginx. UNCONFIRMED.
- **The prover is a second dependency.** Separately from the chain, every write test needs an o1js
  prover (MINA-NOTE §6, D3).

### 2.3 Cardano: Dolos emulator, Yaci DevKit, cardano-testnet

- **Dolos emulator mode** (Apache-2.0, v1.6.2).
  - **How it produces blocks.** Setting `[upstream] block_production_interval = N` (whole seconds),
    instead of a peer, makes Dolos produce its own blocks (DOLOS
    `docs/content/configuration/schema.mdx#L91-L97`). Each block takes up to 10 mempool transactions
    and advances the slot by N. The header is synthetic, with no Praos or VRF
    (`src/sync/emulator.rs#L28-L80`).
  - **Phase-2 runs on submit.** `validate_tx` runs pallas phase-1 and then `evaluate_tx`, and a
    failing redeemer is `Phase2ValidationRejected` (`crates/cardano/src/validate.rs#L66-L131`).
    pallas 1.3.0 is built with feature `phase2` (amaru-uplc).
  - **Packaging and ports.** The binary is 22.2 MB (release tarball), storage can be in-memory, and
    the ports (UTxO RPC 50051, MiniBF 3000, MiniKupo 1442) are configurable.
  - **Funding** comes from the genesis files only. No top-up endpoint was found.
  - **Startup and RAM are UNCONFIRMED** (not documented, not measured).
  - **Clock.** The slot moves only per produced block. No warp was found.
  - CONFIRMED-CODE. This replaces BINDINGS §5.5's "UNCONFIRMED whether it … evaluates Plutus".
- **Yaci DevKit** (MIT, v0.12.0).
  - **Packaging.** The image `bloxbean/yaci-cli:0.12.0` is 931 MB compressed (Docker Hub), and the
    native CLI zip is 65.5 MB. YACI `README.md#L107-L155`.
  - **Two modes** (`config/node.properties#L1-L4`, `docs/pages/yano-node-modes.mdx#L29-L110`):
    - `companion` (the default) bootstraps with Yano, then hands over to the Haskell cardano-node
      11.x, which does phase-2;
    - `yano-only` runs bloxbean's Java ledger with the Scalus evaluator, and is documented as the
      fastest start. Yano calls itself pre-release, "not yet for production validation"
      (YANO `app/README.md#L122-L131`).
  - **Ports and multiple instances.** Every host port is a `HOST_*_PORT` variable, and the compose
    project name is `${node}` (`config/env#L14-L23`, `scripts/docker-compose.yml#L1`), so separate
    stacks look possible. That several native instances (which share `~/.yaci-cli`) can coexist is
    UNCONFIRMED.
  - **Funding.** 20 addresses hold 10k ADA each. `topup`, or
    `POST :10000/local-cluster/api/addresses/topup` (`AddressController.java#L64-L84`).
  - **Clock.**
    - `--block-time` and `--slot-length` go sub-second (default 1 s). `reset`, DB snapshots and
      rollback exist (`docs/pages/commands.mdx#L19-L41`, `#L214-L253`).
    - Yano has `POST /api/v1/devnet/time/advance {slots|seconds|epochs}` and `/devnet/snapshot`
      (YANO `DevnetResource.java#L93-L126`). Whether that is reachable through DevKit in yano-only
      mode is UNCONFIRMED.
  - **Startup and RAM are UNCONFIRMED.** The docs say "starts in seconds"
    (`docs/pages/ci-integration.mdx#L9`), with no RAM figure.
- **`cardano-testnet cardano`** (cardano-node 11.2.0). It defaults to 1 SPO and 2 relays, 0.1 s
  slots, a temp output dir and random gRPC ports. Its README advises a fresh output dir per run
  because "one run poisons the other" (`cardano-testnet/README.md#L70-L255`). Time is wall clock.
  Startup and RAM are UNCONFIRMED. CONFIRMED-DOCS.
- **Inference.**
  - Only the Haskell node (Yaci companion, cardano-testnet) is unambiguously "the real on-chain
    logic". Dolos (Rust UPLC) and Yano (Java/Scalus) are independent reimplementations of the ledger
    rules.
  - Whether a reimplementation counts as real logic or as an emulation under Q4 has to be decided.
    It is the Cardano analogue of anvil, which is itself a reimplementation of the EVM that this
    repository already accepts.

### 2.4 Hedera: Solo

- **Requirements.** Node ≥ 22, Kubernetes ≥ 1.32.2 (kind, which Solo installs itself), and Docker
  with ≥ 12 GB and ≥ 6 cores (SOLO `README.md#L40`). The readiness page asks for 12 GB minimum
  (16 GB recommended), 4 cores and 20 GB disk for a single node
  ([system readiness](https://solo.hiero.org/docs/simple-solo-setup/system-readiness/)). Licence
  Apache-2.0. CONFIRMED-DOCS.
- **Bring-up.** `solo one-shot single deploy` takes "typically … 3–5 minutes" with cached images and
  "10–20 minutes on the very first run". It deploys a consensus node, mirror node, JSON-RPC relay and
  explorer ([quickstart](https://solo.hiero.org/docs/simple-solo-setup/quickstart/)).
  CONFIRMED-DOCS. Not measured.
- **Several instances per host.** One-shot takes `--deployment`, `--namespace` and `--cluster-ref`
  (`src/commands/one-shot/default-one-shot.ts#L67-L90`), and a port that is taken is replaced by the
  next free one ([endpoints](https://solo.hiero.org/docs/using-solo/endpoints/)). That two
  deployments actually coexist is UNCONFIRMED. Each needs ≥ 12 GB, so it cannot happen on a 16 GB
  runner (**Inference**).
- **Solo's own CI.**
  - The PR checks run on custom larger runners (`hiero-solo-linux-medium`;
    `.github/workflows/flow-pull-request-checks.yaml#L51`).
  - One compatibility job runs one-shot deploy/destroy on stock `ubuntu-latest` with a **90-minute**
    timeout (`flow-standard-runner-one-shot.yaml#L18-L84`).
  - CONFIRMED-CODE. **Inference:** it fits on a standard runner, but only just.
- **Account isolation.**
  - Operator `0.0.2` has a well-known key (`src/core/constants.ts#L304-L318`).
  - One-shot pre-creates ECDSA, ECDSA-alias and Ed25519 accounts at 1,000,000 HBAR
    (`predefined-accounts.ts#L42-L94`).
  - Sending HBAR to an EVM address auto-creates a hollow account (HIP-583, Final).
  - The latency to create each account is UNCONFIRMED, and reads also lag by the mirror node
    (BINDINGS §3.4).
- **Clock and shared state.**
  - There is no time-advance command (`src/commands/`).
  - `solo ledger system reset` returns to genesis without redeploying
    (`examples/ledger-reset-smoke/Taskfile.yml#L94-L101`), and how long it takes is UNCONFIRMED.
  - Fees, throttles and the exchange rate are network-wide (**Inference**).
- **hiero-local-node** is deprecated, "to be archived September 2026", yet is still `archived: false`
  today. It uses fixed ports 50211, 5551 and 7546. BINDINGS §3.5 has the rest.
- **The only in-process option is Java.** The consensus node's `EmbeddedNetwork` runs a real
  `Hedera` services instance inside a JUnit process, with a fake platform. `REPEATABLE` mode has
  virtual time (`FakeTime.tick`).
  - It has no gRPC, no JSON-RPC relay and no mirror node
    (`hedera-node/test-clients/README.md#L26-L175` at v0.78.1).
  - It is a test harness, not a product, and is unreachable from Rust without a Java shim.
  - CONFIRMED-DOCS. **Inference:** it runs the real system-contract code, but nothing the connector
    talks to (JSON-RPC) exists in it.
- **What does not count.** `hashgraph/hedera-forking` emulates HTS only and "DOES NOT replicate Hedera
  Token Services fully" (README L14-26), so it fails Q4.

---

## 3. Several instances on one host

| Chain                 | Native or Docker       | Port control                                  | Fixed names or paths                                     | Verdict                                |
| --------------------- | ---------------------- | --------------------------------------------- | -------------------------------------------------------- | -------------------------------------- |
| anvil                 | native                 | `--port` (or `-p 0`)                          | none                                                     | any number                             |
| solana-test-validator | native                 | RPC, faucet and dynamic range by flag         | `--ledger` temp dir                                      | any number (CI ran 15 at once)         |
| xrpld                 | native (apt) or Docker | config file                                   | DB and log paths, set in config                          | any number (Inference)                 |
| Mina lightnet         | Docker only            | `-p` remap                                    | zkapp-cli's container name (avoid zkapp-cli)             | ≥ 2 MEASURED; RAM-bound                |
| Dolos                 | native                 | `listen_address` per service                  | storage dir, or in-memory                                | any number (Inference)                 |
| Yaci DevKit           | Docker or native CLI   | `HOST_*_PORT` variables; compose project name | `~/.yaci-cli` for native                                 | Docker: plausible; native: UNCONFIRMED |
| Solo                  | kind (Docker) + Helm   | auto next-free port                           | cluster `solo-cluster`, deployment `one-shot` by default | one per 16 GB runner (Inference)       |

- **How cargo schedules tests.** Cargo ran RUN's test targets serially. Inside a target, libtest
  threads run tests in parallel, and the default thread count is the CPU count. CONFIRMED-CODE (RUN
  log).
- **Parallel CI jobs.** Each job gets its own VM, so parallel jobs do not share a host
  ([runners reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)).
  CONFIRMED-DOCS.
- **Inference.** On a shared tier, "one chain per test binary" means one chain at a time on the gate
  runner, because targets run serially. A developer running two `cargo test` invocations at once
  would get two, so ports must still be per-instance.

## 4. Clocks (BINDINGS D7)

| Chain         | Move time?                                              | Global?                                  | Forces own chain?                                                  |
| ------------- | ------------------------------------------------------- | ---------------------------------------- | ------------------------------------------------------------------ |
| anvil         | `evm_increaseTime`, `evm_setNextBlockTimestamp`         | yes                                      | yes (today: the tests in §1.1)                                     |
| Solana        | `--warp-slot` at spawn only                             | n/a after spawn                          | only by needing a spawn flag                                       |
| xrpld         | ≥ 1 s per `ledger_accept`; no direct set                | yes, and any `ledger_accept` advances it | every delay test; arguably every test that relies on ledger timing |
| Mina lightnet | `SLOT_TIME` at start; no warp found                     | yes                                      | delay tests become real-time or move off the gate                  |
| Dolos         | slot += interval per block; no warp found               | yes                                      | as Mina                                                            |
| Yaci (Yano)   | `devnet/time/advance` (Yano API), snapshot and rollback | yes                                      | yes, if used                                                       |
| Solo          | none                                                    | yes                                      | delay tests are real-time                                          |

**Inference.** On a shared XRPL node, `ledger_accept` is both "make my transaction final" and "move
everyone's clock". So no XRPL test is clock-neutral, and XRPL is a weak candidate for the shared
tier. That matters little, because XRPL fits per test anyway.

## 5. GitHub Actions runners

- **Visibility.** `toon-protocol/connector` is **public** (`gh repo view`: `"visibility":"PUBLIC"`).
  CONFIRMED-CODE.
- **Standard Linux x64 runners.**
  - Public repositories get **4 CPU, 16 GB RAM and 14 GB SSD**; private ones get 2 CPU, 8 GB and
    14 GB. `ubuntu-latest` is 24.04
    ([runners reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)).
  - Standard runners are free for public repositories
    ([Actions billing](https://docs.github.com/en/billing/concepts/product-billing/github-actions)).
  - CONFIRMED-DOCS.
- **Preinstalled on the 24.04 image.** Docker 28.0.4, Compose 2.38.2, **kind 0.33.0**, kubectl
  1.37.1 and Helm 3.22.0 (runner-images readme L34, L74-90). CONFIRMED-DOCS.
- **Kind on a standard runner.** helm/kind-action's README says that "GitHub-hosted `ubuntu-latest`
  runners satisfy" its Docker requirement
  ([kind-action](https://github.com/helm/kind-action/blob/4852af461bb6e2471dcf739df45a52672b857043/README.md)).
  No first-party RAM figure for kind on Linux CI was found. UNCONFIRMED.
- **Larger runners.**
  - Linux sizes are 2/8, 4/16, 8/32, 16/64, 32/128 and up to 96 vCPU / 384 GB
    ([larger runners](https://docs.github.com/en/actions/reference/runners/larger-runners)).
  - Quote: "Larger runners are always charged for, even when used by public repositories."
  - They need GitHub Team or Enterprise Cloud and a payment method.
  - CONFIRMED-DOCS.
- **Limits.** The default job timeout is 360 min, and the free plan allows 20 concurrent jobs
  ([limits](https://docs.github.com/en/actions/reference/limits)). CONFIRMED-DOCS.
- **Inference on the 16 GB budget.**
  - The gate job also compiles the workspace, which this note did not measure.
  - Solo's 12 GB minimum leaves about 4 GB for the rest.
  - One lightnet (0.7 GB measured; 4.5 GB to start per docs) fits.
  - Fifteen parallel Solana validators at about 380 MB each come to about 5.7 GB, which already
    fits only because of the 16 GB size.

## 6. Prior art

- **One process per test is what the Rust ecosystem's chain harnesses do.**
  - The `bitcoind` crate spawns per `BitcoinD::new`, with a fresh `TempDir`, and a port from binding
    `:0`; its doc admits "there is a race condition". Drop kills the process
    ([`corepc@8d82c88` `bitcoind/src/lib.rs` L240-663](https://github.com/rust-bitcoin/corepc/blob/8d82c88aebf8268bfc9732aaa15e64a5bba1f9a0/bitcoind/src/lib.rs)).
  - `electrsd` does the same
    ([`lib.rs`](https://github.com/RCasatta/electrsd/blob/d2fd8afc3b5b79304dd669250df325a54024f374/src/lib.rs)).
  - alloy's Anvil uses `-p 0` and parses the port (§1.1).
  - CONFIRMED-CODE.
- **Per-test Docker chains.** interchaintest creates a randomly named Docker network per test,
  labels everything with `t.Name()`, and prunes in `t.Cleanup`
  ([`cosmos/interchaintest@43a1d4b` `dockerutil/setup.go`](https://github.com/cosmos/interchaintest/blob/43a1d4b4a12646d32ff51c053a45ad04d40a3cd8/dockerutil/setup.go)).
  It runs on standard `ubuntu-latest` with `-timeout 30m -p 2`, and its recent e2e runs took about
  20 min (from `gh run list` timestamps). CONFIRMED-CODE.
- **One stack per run or suite.**
  - Hyperlane's `run-locally` starts one anvil and one `solana-test-validator` per run, and
    `pkill -SIGKILL anvil`s first outside CI because "it seems to have issues getting cleaned up".
    Its CI uses a paid 8-core third-party runner
    ([`ethereum/mod.rs` L19-51](https://github.com/hyperlane-xyz/hyperlane-monorepo/blob/a9b3e8705816eda0e4e84b03592cadf944d8ce43/rust/main/utils/run-locally/src/ethereum/mod.rs#L19-L51),
    [`test-rust-e2e.yml`](https://github.com/hyperlane-xyz/hyperlane-monorepo/blob/a9b3e8705816eda0e4e84b03592cadf944d8ce43/.github/workflows/test-rust-e2e.yml)).
  - LayerZero devtools shares one docker-compose stack per suite, on `ubuntu-latest-16xlarge` larger
    runners
    ([`docker-compose.yaml`](https://github.com/LayerZero-Labs/devtools/blob/4973ba8bef7b0fdf7268469abea3ea50dbd4bbd8/docker-compose.yaml),
    [`reusable-test.yaml`](https://github.com/LayerZero-Labs/devtools/blob/4973ba8bef7b0fdf7268469abea3ea50dbd4bbd8/.github/workflows/reusable-test.yaml)).
  - chainlink-testing-framework uses fixed container names and same-port mapping, so one suite runs
    per host, plus opt-in component caching
    ([`solana.go` L102-128](https://github.com/smartcontractkit/chainlink-testing-framework/blob/32538607cda4426bf9b7a2560e11e53528c4b894/framework/components/blockchain/solana.go#L102-L128)).
  - CONFIRMED-CODE.
- **Reuse within a process.** testcontainers-rs `ReuseDirective::CurrentSession` labels a container
  with a per-process ULID and finds it again. It is **experimental**, behind `reusable-containers`,
  and a reused container "will not be stopped or cleaned up" on drop. Host ports are random by
  default. An optional `watchdog` feature stops containers on SIGTERM, SIGINT or SIGQUIT
  ([`image_ext.rs` L17-230](https://github.com/testcontainers/testcontainers-rs/blob/86982cd11e59228fab8507dfd1a3422490518ba6/testcontainers/src/core/image/image_ext.rs#L17-L230),
  [`watchdog.rs`](https://github.com/testcontainers/testcontainers-rs/blob/86982cd11e59228fab8507dfd1a3422490518ba6/testcontainers/src/watchdog.rs)).
  CONFIRMED-CODE.
- **"One shared node per test binary, fresh accounts per test" has no first-party write-up** in any
  project surveyed. UNCONFIRMED (searched, not found). Lightnet's accounts manager, built for
  "parallel automated tests execution" against one network (MINA-NOTE §6), is the closest
  maintainer-endorsed design.
- **Inference: what the shared tier must solve that per-test spawning does not.**
  - **Teardown.** A `static` is never dropped, so the child process or `docker run` container
    outlives the test binary. Options:
    - Linux `PR_SET_PDEATHSIG` on the child;
    - `docker run --rm` plus a label that a pre-run reaper cleans up;
    - a testcontainers-style watchdog, which covers signals but not `SIGKILL`.
    - The gate's VM dies with the job; a developer's machine does not.
  - **Runtimes.** Each `#[tokio::test]` builds its own runtime. A chain handle in a `OnceLock` must
    therefore hold no runtime-bound resources: a URL and a process handle, not a client.
  - **Failure attribution.** One panicking test can leave the chain in a state that other tests then
    see.

---

## 7. The candidates in one table

"Per test" means the chain fits the current rule as written. "Shared" means it fits the owner's
shared tier: harness-spawned, lazy, fails CI loudly, real logic.

| Chain                            | Fits per test?                           | Fits shared tier?                                            | Startup to usable                                      | RAM                                      | Several per host                       | Account isolation                                                 | Clock control                                    | Notes                                                                           |
| -------------------------------- | ---------------------------------------- | ------------------------------------------------------------ | ------------------------------------------------------ | ---------------------------------------- | -------------------------------------- | ----------------------------------------------------------------- | ------------------------------------------------ | ------------------------------------------------------------------------------- |
| anvil (baseline)                 | yes                                      | n/a                                                          | 0.03 s (MEASURED)                                      | ~95 MB                                   | yes                                    | genesis keys; `anvil_setBalance`                                  | full warp                                        | today                                                                           |
| solana-test-validator (baseline) | yes (today)                              | yes                                                          | ≤ 4.7 s on CI v2.1.21; 13.6 s to first slot on v3.1.12 | ~380 MB                                  | yes                                    | `requestAirdrop`, fresh `Keypair`                                 | `--warp-slot` at spawn                           | 64 % of test time in RUN                                                        |
| XRPL `xrpld` stand-alone         | **yes**                                  | yes, but every `ledger_accept` is global                     | 1.0–3.2 s (MEASURED, Docker)                           | 72–322 MiB                               | yes (config per instance)              | Payment from genesis + accept, ~0.23 s; races on genesis sequence | ≥ 1 s per accept; ~900 accepts for a 901 s floor | apt package or 106 MB image; ISC; XRP-only channels (BINDINGS §2.3)             |
| Mina lightnet (proofs off)       | no (37–46 s, plus ~26 s per funding)     | **yes**                                                      | 37–46 s to SYNCED (MEASURED); docs say 1–2 min         | 0.6–0.7 GiB running; 3.5–4.5 GB to start | yes, via `-p` remap (MEASURED ×2)      | accounts manager, ~1000 accounts, ~1 s acquire                    | `SLOT_TIME` only; no warp found                  | Docker only; ~0.9 GB image; proofs off                                          |
| Mina devnet image (proofs on)    | UNCONFIRMED                              | UNCONFIRMED                                                  | UNCONFIRMED                                            | UNCONFIRMED                              | presumably as lightnet                 | presumably as lightnet                                            | real slots                                       | the only local option with `PROOF_LEVEL=full`                                   |
| Cardano Dolos emulator           | plausibly (22 MB binary, in-memory)      | yes                                                          | UNCONFIRMED                                            | UNCONFIRMED                              | yes (configurable listeners)           | genesis UTxOs only; no top-up found                               | interval per block; no warp                      | Rust UPLC phase-2; synthetic headers; Q4 status open                            |
| Cardano Yaci DevKit              | unclear (931 MB image; native 65 MB CLI) | yes                                                          | "seconds" (vendor), UNCONFIRMED                        | UNCONFIRMED                              | Docker: plausible; native: UNCONFIRMED | 20 funded addresses + `topup` REST                                | sub-second slots; Yano `time/advance`, snapshots | companion = Haskell node (real); yano-only = Java reimplementation, pre-release |
| Hedera Solo                      | no                                       | **no as written**: kind + Helm, ≥ 12 GB, 3–5 min warm (docs) | 3–5 min warm, 10–20 min cold (docs)                    | ≥ 12 GB                                  | not on a 16 GB runner                  | operator 0.0.2; pre-made accounts; hollow accounts                | none                                             | Solo's own PR CI uses custom larger runners                                     |
| Hedera EmbeddedNetwork           | n/a from Rust                            | no (no JSON-RPC, Java-only)                                  | —                                                      | —                                        | —                                      | —                                                                 | virtual time in REPEATABLE                       | real services code; not a product                                               |

---

## Unconfirmed, and why

- **Exact chain spawns per `cargo test --workspace`.** This is a static estimate. Counting would
  have meant running the workspace, which this note avoided.
- **Mina's `-devnet` image with `PROOF_LEVEL=full`** (startup, RAM, block time). It was not pulled,
  and no first-party figure exists.
- **Dolos and Yaci startup and RAM.** Neither is documented, and neither was run.
- **Solo's measured bring-up and RAM on a 16 GB standard runner**, the duration of
  `ledger system reset`, and per-account creation latency. Only the docs and Solo's own workflow
  files were read.
- **Kind's memory floor on Linux CI.** No first-party figure was found.
- **A warp for the Mina global slot or Dolos time.** Searched, not found.

## Decisions this forces

These are questions, not answers.

1. **The threshold.**
   - What makes a chain "too costly per test"? Candidate measures are seconds to first usable block,
     RSS per instance × the widest test target's parallelism (15 today) against the 16 GB runner, and
     total added gate minutes.
   - Today's own Solana validator (≤ 4.7 s on CI, ~380 MB) sits where on that scale? Does the rule
     apply retroactively, and would it move Solana to the shared tier?
2. **The doctrine text.** Does the amending ADR also reconcile ADR 0007's "per run" with CLAUDE.md's
   "per test" (§1.5), and cite 0007's own risks as the reason a per-binary chain with fresh keys is
   acceptable?
3. **Teardown.** How does a `static` chain die with its test binary: `PR_SET_PDEATHSIG`,
   `docker --rm` plus a labelled reaper, or a watchdog? Is a leaked chain on a developer's machine
   acceptable?
4. **Docker in `require_*`.**
   - A shared-tier chain for Mina, Solo and possibly Yaci is Docker-only. Does the fail-in-CI rule
     cover "Docker present, image pulled, digest pinned", and is the image pinned by digest the way
     Foundry and Solana are pinned by version?
   - Does a local run without Docker skip, like a missing `anvil`?
5. **"Real on-chain logic" (Q4) for reimplementations.** Anvil is already one. Do Dolos (Rust
   UPLC/pallas) and Yaci's yano-only (Java/Scalus) qualify, or only Yaci's companion mode with the
   Haskell node?
6. **Mina's proofs-off fidelity.** A default lightnet cannot verify proofs (§2.2). Is a proofs-off
   shared lightnet admissible as the gate, given that MINA-NOTE D11 says it must not be the only VK
   check? If not, what does the `-devnet` image cost, and where is the proofs-on tier allowed to run
   (on the gate, or off it under Q3's "beside a gate-passing tier 3")?
7. **Hedera's CI cost.** Solo needs ≥ 12 GB and kind on a 16 GB public runner that also compiles the
   workspace. Options:
   - a separate CI job with its own runner, which is free but has to be measured first;
   - a paid larger runner (Team plan or above);
   - not admitting Hedera, because no harness-spawned real-logic chain fits.

   Does "spawned by the harness" stretch to a harness that shells out to `solo one-shot` and kind?

8. **Clock-bound tests on chains without a warp.** On Mina, Dolos and Solo, delay tests become
   real-time (a 901 s floor, BINDINGS D7) or need a per-chain test-only floor. Which is it? And is
   XRPL's ~900 `ledger_accept`s per delay test acceptable on its own chain?
9. **XRPL on a shared chain.** Since every `ledger_accept` is global, should XRPL simply be pinned to
   per test (it fits), so the shared tier is never used for it?
10. **Funding contention.** On a shared chain, funding comes from one genesis account (XRPL, Dolos,
    Yaci) or a pool (Mina). Must the harness serialise funding behind a mutex or pre-split accounts
    at spawn, so that genesis-sequence races do not look like the nonce bugs ADR 0007 warns of?
