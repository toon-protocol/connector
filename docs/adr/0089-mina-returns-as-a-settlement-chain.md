# Mina returns as a settlement chain

**Status:** Accepted (owner decisions, 2026-10-28, on map #1548's children #1549–#1561; written up 2026-11-05, #1660) — **not yet built**. Building it waits on toon-protocol/zk_toon#26 (the channel zkApp, its address, VK hash, map height and batch size) and zk_toon#32 (recursion in the Rust prover). It **amends** [0002](0002-drop-mina-from-the-rust-connector.md) (Mina is a settlement chain again, proved in Rust and with no sidecar), [0065-mina](0065-mina-leaves-the-repository.md) (the surviving `mina` refusal narrows to the client edge) and [0080](0080-a-client-may-pay-from-a-zk-note.md) decisions 1 and 7 (the same narrowing, and a `mina:` asset in `[[tokens]]` and a settlement table). It **keeps** the o1js falsifiers of 0065-mina and 0080 exactly as they stand. It **applies** [0085](0085-whose-code-a-channel-may-bind-to.md): the zkApp is code of kind 3 (TOON's), and the upgrade key is decision 5's dormant-key case, which this record names. 0075 decision 1's amendment for Mina (#1558) was already recorded by 0085, so it isn't made again here.

**Scope:** protocol law. A Mina voucher, its claim-state challenge, the asset id and the vectors bind every implementation. The prover, the backend and the test harness are connector architecture. See the [ADR index](README.md).

**Falsifier:** `crates/**/Cargo.toml` matching `^name = "connector-settlement-mina"` — the Mina backend crate exists, so this Status line's "not yet built" has gone stale.

**A node may settle a peering on Mina, in native MINA, over x402 batch-settlement channels held
by a TOON-owned zkApp in toon-protocol/zk_toon. The connector proves and submits that zkApp's
transactions itself, in Rust, with no o1js at run time and nothing running beside it. A Mina
voucher is accepted on a peer carriage only. The client edge still refuses `blockchain: "mina"`
by name. After each Mina hard fork, one dormant key installs one verification key that the
connector pins in advance.**

## Context

[0002](0002-drop-mina-from-the-rust-connector.md) dropped Mina because a channel's on-chain steps
are zkApp calls, a zkApp call needs a proof, and only o1js (JavaScript) made one. It refused a Node
sidecar, and refused to port the circuit to Rust because a verification-key mismatch "is
undetectable until a transaction is rejected on-chain". [0065-mina](0065-mina-leaves-the-repository.md)
then deleted the old zkApp and everything else Mina-related, apart from the connector's refusal of a
`mina` claim, which is owed to `toon-client`. [0080](0080-a-client-may-pay-from-a-zk-note.md)
brought Mina back for one purpose only: a client may pay its **first** hop from a zk note in a Mina
vault. It said: "There is still no Mina channel, no Mina settlement backend, and no sidecar."

The owner wants peerings to settle in MINA the way they settle on EVM, Solana and XRPL: two x402
batch-settlement channels per peering ([0075](0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md)),
paid with cumulative vouchers. That covers every hop **after** a zk edge. A zk note needs no
channel, and peers are known operators who need no vault.

Map #1548 settled every design question on 2026-10-28. This record writes those answers down. Four
facts changed since 0002 was written:

- **Proving in Rust works** (#1561, [spike](../research/rust-zkapp-prover-spike.md)). o1js exports,
  at compile time, exactly the circuit artefacts mina-rust's prover loads. Kimchi proved o1js's
  `land` step circuit in 0.95 s, against o1js's 4.7–5.3 s. #1512's Rust verifier accepted o1js's
  real proof.
- **x402 has no Mina binding** ([research](../research/mina-x402-batch-settlement.md)), so the
  on-chain code is TOON's. #1558 admitted that, and 0085 recorded it as kind 3.
- **`network.timestamp` is broken on Mina mainnet** after the Mesa fork. o1js #1588 is open, and
  the error is about 2.7 years. The old zkApp's exact-`globalSlot` bug, which 0002 retired, has a
  modern cousin ([research](../research/mina-x402-binding-answers.md) D4).
- **Mina carries about 2.9 zkApp commands a minute**, and a block reaches 15 confirmations in about
  an hour ([research](../research/mina-settlement-confirmation-and-fees.md)). Payments are
  off-chain vouchers, so this limits only bring-up, top-ups, landing and fees.

## Decision

### 1. Mina is a settlement chain again, for peerings, in native MINA

- **The asset is native MINA**: token id 1, 9 decimals, amounts in `u64` nanomina. Its asset id
  is the CAIP-19 `mina:<network>/slip44:12586` everywhere inside the connector, as 0080 decision 7
  (amended by the merge of #1607) and [0086](0086-a-node-settles-on-more-than-one-evm-network.md)
  decision 2 already spell it. `mina:native` stays refused by name.
- **`[[tokens]]` and a `[settlement.mina]` table may name it.** This amends 0080 decision 7's
  refusal of a `mina:` asset "in `[[tokens]]` or a settlement table". That refusal stood only
  because Mina had no settlement backend. `AssetChain::Mina` already exists for the zk-note vault.
  What changes is the refusal it carries in `[[tokens]]` and the settlement tables.
- **Every Mina leg is a [0071](0071-a-forward-crosses-a-denomination-at-a-declared-rate.md)
  dealing boundary**, with a static `[[rates]]` row and a spread, under
  [0087](0087-xrpl-settles-over-the-ledgers-payment-channels.md) decision 6's native-coin rules. A
  channel holds MINA only, and no pool a TWAP source can read quotes MINA (0080 decision 7).
- **A custom Mina token is deferred.** The MINA zkApp asserts `tokenId == 1`. A token channel
  would be a separate zkApp with its own VK, with three keys able to install a VK at a fork and an
  admin able to pause transfers (#1559). It is designed only when a token is worth carrying.
- **Peers settle over channels; clients pay over zk notes.** Nothing here gives a client a Mina
  channel (decision 4).

### 2. The binding is TOON's, pinned at a commit, and proposed upstream

- **The binding is zk_toon's `docs/design/scheme_batch_settlement_mina.md` at `cafb15cf`**
  (toon-protocol/zk_toon#34, merged 2026-10-28). It is TOON's own binding of x402's chain-neutral
  `batch-settlement` outline, built on the EVM binding's shape.
- **0085 decision 2 applies as it does to someone else's open PR.** The connector builds to that
  commit and doesn't track the file loosely. A later change is a reviewed change to this record. A
  change that moves claim bytes bumps the vectors' `schema_version`
  ([0021](0021-vectors-are-normative-prose-is-not.md)).
- **The binding is being proposed upstream, and TOON doesn't wait for a merge** (#1558). The
  discussion is open as x402-foundation/x402#4043. The spec-only PR (#1562) is opened only on the
  owner's go-ahead, after zk_toon#26 has run the mechanics on lightnet.
- **The draft makes three choices for review that no ticket fixed:** the upstream `asset`
  spelling, the `networkId` values, and `deposit.transaction`'s encoding. The vectors pin whichever
  stands. The rest of the draft's TOON profile, above its marker, is decided by the tickets this
  record cites.

### 3. The zkApp lives in zk_toon, and the connector proves in Rust

- **The channel zkApp is code of 0085's kind 3** (#1550). It lives in toon-protocol/zk_toon,
  beside the zk-note vault, and nothing of it lives here. The connector binds to it by address and
  VK hash, as it binds to `x402BatchSettlement` and `payment-channels`.
- **The Mina backend proves and submits in-process, in Rust** (#1551). It uses zk_toon's prover:
  the wrap prover (zk_toon#30, ported once per Pickles version), the step-witness generator
  (zk_toon#31, once per method) and recursion for the BatchReducer (zk_toon#32). The connector
  depends on these as Rust crates. o1js stays a build-time tool inside zk_toon.
- **0002's refusal of a sidecar stands.** No Node process and no o1js submitter run beside the
  connector. 0065-mina's `**/package.json` falsifier and 0080's `**/Cargo.toml` and
  `**/package.json` falsifiers stand unchanged, and still hold, because o1js never enters this
  repository's manifests.
- **0002's reasoning is corrected in one place.** Its "a VK mismatch is undetectable until
  rejected on-chain" doesn't survive. The prover loads o1js's own compiled circuit, and each Rust
  witness and proof is checked against o1js's in a test. **Its maintenance worry does survive, in a
  narrower form.** The Rust side mirrors o1js's gadget implementations, which can change on a
  minor o1js release with no fork. The pinned VK hash catches the drift, but the mirror has to be
  redone before the zkApp can upgrade.
- **Every proof finishes before its command is submitted, inside the command's `valid_until`**
  (decision 6). zk_toon fixes the reducer's batch size so that a worst-case batch proves within 5
  of the 20 slots on a named reference machine (#1553).

### 4. The wire: a Mina voucher is accepted on a peer carriage only

- **The `mina` refusal narrows to the client edge.** Today `parse_client_claim` refuses
  `blockchain: "mina"` by name **before** it reads `scheme`, on every carriage, so no Mina voucher
  could reach a backend. After this record, a Mina `batch-settlement` voucher is accepted on a
  peer carriage, on a bound channel. At the client edge it is still refused by name. This is the
  refusal 0065-mina left standing as "wire behaviour owed to `toon-client`", and it stays owed
  there. This amends 0080 decision 1 and 0065-mina's closing paragraph, and the error text stops
  citing 0002 as its reason.
- **The voucher** is a Mina Schnorr signature over Pallas, under the `CodaSignature*******` domain
  on every network, with o1js's chunked nonce and network id `0x00`. It signs one field: Poseidon
  `hashWithPrefix("x402-mina-voucher***", …)` over the channel id and the cumulative amount. The
  binding fixes the field order. The amount is cumulative, with no nonce and no expiry, so 0075's
  amount-only watermark applies unchanged. Above `u64::MAX` nanomina it is refused as
  `AmountExceedsChain`. The port stays `u128`.
- **Every tag is exactly 20 bytes, padded with `*`.** o1js pads with zero bytes while Mina's
  OCaml and Rust pad with `*`, so a shorter tag hashes differently across the two.
- **The channel id** is `hashWithPrefix("x402-mina-channel***", …)` over the channel's config,
  the zkApp address and a **network id that is a TOON constant**: `0` devnet, `1` mainnet. It is
  never Mina's `chainId`, which changes at every hard fork.
- **The claim-state challenge** ([0075](0075-every-channel-is-an-x402-channel-a-peering-is-two-of-them.md)
  decisions 5–6) is TOON's own, under its own tag `x402-mina-challenge*`, and isn't proposed
  upstream. It is signed under the channel's voucher key, like a voucher.
- **Vectors:** peer-carriage cases only, which are a Mina voucher, the Mina claim-state challenge
  and a channel-id case, at the next `schema_version`. There is no client-edge Mina section,
  because the client edge refuses Mina.
- **Signer:** `VoucherSigner` gains a Mina variant, published in the self-description's
  `voucherSigners` as a `B62…` address.
- **The greeting** carries one `batch-settlement` entry for the table: `network` `mina:mainnet` or
  `mina:devnet`, and `asset` `slip44:12586`, composing the CAIP-19 id as 0080's zk-note entry does.
  Its `extra` names the zkApp address and VK hash, and a peer checks both against its own binary's
  constants (decision 7). Terms are told apart by their `network` prefix.

### 5. Storage and authorization on chain

These belong to the zkApp, which is zk_toon's. They are recorded here because the backend builds
to them and the connector's safety rests on them (#1553).

- **One shared zkApp account.** Methods only dispatch actions. A permissionless, recursive
  BatchReducer folds them in order into an `IndexedMerkleMap` keyed by `channelId`. Direct writes
  to a shared root were rejected: every write would invalidate every other in-flight proof, which
  allows roughly one channel write per block across all of Mina.
- **`reduce` is total.** Every action either applies or is recorded as a no-op. A stale claim,
  an over-balance claim, an early finalize and a stale refund are no-ops, never a revert. Anyone
  may reduce. A connector reduces when one of its own actions is pending, and pays the fee.
- **Claim and settle are one `claim`**, authorized by an in-circuit `receiverAuthorizer`
  signature, which anyone may prove and submit. Deposit, top-up, `initiateWithdraw` and
  `finalizeWithdraw` are the payer's signed child updates. `refund` keeps its signed
  `(channelId, nonce, amount)`.
- **Payouts are pulled.** `reduce` credits a withdrawable balance per address, and `collect`
  sends it. A pushed payout would let a recipient with `receive: impossible` jam every operator's
  queue. A connector bundles `collect` right after its own `reduce`, in the same command, proved
  against the root that reduce produced. It skips a collect worth less than the collect's fee.
- **Time comes only from `currentSlot.requireBetween(lo, hi)`**, with `hi` no later than the
  command's `valid_until`. Nothing uses `network.timestamp` and nothing requires an exact slot.
  The withdraw delay is in seconds in config and is converted in-circuit as `ceil(secs / 90)`.
  The 90 s slot is a VK constant, so a change in slot length is an upgrade (decision 7).
- **Landed and counted are measured at the dispatch**, not at the reduce.
- **The connector rebuilds the map** from an untrusted `archive_url` and checks the result
  against the on-chain action state and root. An archive that answers wrongly fails closed.
- **Formats are frozen across VK installs.** These are the action encoding, the leaf and its hash,
  the payout key derivation and the map height. A change to any of them means a new zkApp and a
  drain, never an install.

### 6. Confirmation, waits and fees

From #1560:

- **Landed** means included in a best-chain block with an empty `failureReason`. **Counted**
  means 15 blocks deep, measured in blocks. Only counted collateral backs vouchers, on both sides.
  Nothing is carried on credit, so a new Mina peering carries no packet for about an hour.
- **The withdraw delay's floor and default are both one day.** The Mesa fork stopped transactions
  for 8 hours, which rules out a shorter floor. An incoming channel below the floor is refused.
  The binding also requires `withdrawDelay ≥ maxTimeoutSeconds`.
- **Vouchers on open channels are landed every 24 hours**, a constant. `POST /channels/:id/land`
  covers anything sooner, and the watcher lands at once when a withdrawal lands.
- **Writes return at pool admission.** `POST /peers` and `POST /channels/:id/fund` answer `202`
  once the pool accepts the command, with the channel `Opening`. Every command carries
  `valid_until` = submit slot + 20, so "not landed" becomes definitive and resubmitting is safe.
  A watcher moves the channel to `Open` with pending collateral, and then to counted.
- **Fees:** every command pays 0.1 MINA. One that isn't included within 3 blocks is replaced at
  the same nonce at double the fee, capped at 1 MINA. A replacement needs a new fee-payer
  signature but no new proof. Landing vouchers ahead of a withdrawal starts at 1 MINA. The node
  that sends a command pays its fee from its Mina settlement key.
- **There is no facilitator.** The binding defines an optional one upstream, and TOON doesn't run
  it. [0076](0076-the-operator-names-the-facilitator-and-pays-its-gas.md) stays EVM-only.

### 7. Hard forks, the upgrade key, and the trust statement

From #1559, and 0085 decision 5's dormant-key case, named here as that decision requires:

- **Permissions.** `access: proof`, `setVerificationKey: impossibleDuringCurrentVersion`, `proof`
  for `editState`, `send` and `editActionState`, and `impossible` for everything else. After a
  protocol-version change, Mina's consensus code falls `setVerificationKey` and `access` back to
  `signature`, and nothing else falls back.
- **The upgrade key is the zkApp account's own key.** It is dormant between forks. After a fork it
  installs one VK and does nothing else. On devnet it is a single key held offline. **Before any
  mainnet deployment it is a 2-of-3 FROST group key** (MMS `frost-bluepallas`). 0085 decision 3's
  table holds a mainnet Mina row only once that is so, and the record adding the row names the key
  and its threshold.
- **The install is checkable off chain.** The post-fork VK is built reproducibly from a tagged
  zk_toon commit and published with its source diff before the stop slot. The connector release
  pins that hash, the installer installs only that hash, and anyone can compare the installed hash
  with the published one.
- **What the connector pins** is a binary constant per network: the zkApp address and the pair
  (VK hash, `setVerificationKey.txnVersion`). The hash alone isn't enough, because a pre-fork VK
  stays on chain until the install. The pin is checked at boot and before each Mina write. A
  mismatch fails closed **for the Mina backend only**, and EVM, Solana and XRPL carry on. Each
  release is built for one Mina era. The rollout is an ordinary release cut ahead of the fork,
  then each node repository's own pin bump ([0068](0068-a-node-repository-pins-the-connector-nothing-here-moves-a-tag-onto-a-box.md)).
- **Across a fork:** before the announced stop slot, the backend lands every voucher it holds that
  isn't yet on chain. From the stop slot until the install, everything is frozen. The installer
  sends the VK update and `resume()` in one command. `finalizeWithdraw` requires
  `currentSlot ≥ resumedAt + minDelay`, so a receiver always gets a full delay after the thaw.
  zk_toon#35 confirmed on lightnet that a VK installed earlier in a command verifies a later proof
  in it.
- **The trust statement**, in the shape of [0074](0074-a-client-may-pay-over-an-x402-batch-settlement-channel.md) decision 5. After a fork, the key holder can install any
  circuit, including one that pays the escrow to itself. Funds are frozen until the install. One
  wrong install lasts until the next fork. **The Mina backend is operator opt-in**: a node without
  a `[settlement.mina]` table accepts none of this.

### 8. Configuration and keys

- **`[settlement.mina]`** carries the network (`mina:devnet` or `mina:mainnet`), `rpc_url`,
  `archive_url` and `min_withdraw_delay_secs`. A network with no row in 0085 decision 3's table is
  refused by name. This record admits one Mina network per node; a second waits for its own
  decision under 0086.
- **`[settlement.mina.key] key_file`** (`settlement-mina.key`) holds a 32-byte Pallas scalar, as
  raw bytes or 64 hex characters, referenced by path ([0009](0009-one-typed-config-file-no-environment-layer.md),
  [0012](0012-a-signer-and-a-treasury-not-a-wallet.md)). On a node's own channels it plays every
  role: payer, `payerAuthorizer`, receiver, `receiverAuthorizer` and `payTo` (0075 decision 3). It
  also pays every fee.
- **Settlement RPC is built as every chain's is.** Every client of the table's `rpc_url`, and of
  its `archive_url`, is built from one `connector_chain_rpc::RpcTransport` in
  `runtime::settlement_transports`, and may opt into the `socks_proxy` under
  [0073](0073-settlement-rpc-may-ride-the-circuit-once-every-wait-on-it-is-bounded.md).

### 9. The suites and tier-3 tests

- **Parity is #1565's option (b).** The `batch::contract` suites keep their assertions, and Mina
  is fitted to them:
  - amounts above the ceiling are refused as `AmountExceedsChain`;
  - the fixture's `payer_balance` adds fees back;
  - `let_delay_pass` really waits out a short delay, because a lightnet has no movable clock.

  A lifecycle fact that doesn't hold on EVM and Solana is tested beside the suites, not in them.

- **The chain is a lightnet, one per test binary** (#1554), under
  [0084](0084-a-chain-too-heavy-to-spawn-per-test-is-spawned-once-per-test-binary.md)'s shared
  tier. This is that tier's first user. The harness spawns it, Docker only and pinned by digest,
  and labels it `toon.test-chain.owner`. The accounts manager is the single funder.
- **Each test deploys its own zkApp** from the pinned VK, then asserts that the account's VK hash
  and permissions equal decision 7's.
- **Chain tests submit the constant dummy proof.** It is gated to `test-support` and kept out of
  the prover port's contract suite.
- **A chainless gate test proves every method the backend invokes** with the real Rust prover,
  and verifies each proof under the pinned VK.
- **Delay tests use a short `SLOT_TIME`** and delays of a few slots, with no test-only floor.
- **`require_mina()` panics in CI** when any of these holds:
  - Docker is missing;
  - the image isn't at the pinned digest;
  - the chain isn't `SYNCED` within 180 s;
  - `/acquire-account` doesn't answer.
- **The harness is built in the first Mina backend crate's `test_support`.** CLAUDE.md's tier-3
  wording changes in that PR.

## Considered options

- **A Node sidecar running o1js.** Rejected by 0002, and still rejected. The point of the Rust
  connector is to carry no JavaScript runtime.
- **An o1js submitter beside the connector** (#1551 option 2). Rejected for the same reason. The
  spike showed it isn't needed.
- **A zk note vault for peers.** Rejected. Peers are known operators to each other, and a vault
  buys privacy a peering doesn't need.
- **One zkApp account per channel, or a token account per channel.** Rejected (#1553, #1559). Each
  account needs its own install at every fork, under its own key.
- **Writing channel state directly to the root.** Rejected (decision 5). It allows about one
  channel write per block across all of Mina, and every loser still pays its fee.
- **Pushed payouts.** Rejected (decision 5). One recipient's permissions could jam everyone's
  queue.
- **`network.timestamp` preconditions.** Rejected. They are wrong on mainnet since Mesa, and they
  are checked against the parent block (o1js #1588).
- **Binding Mina's `chainId` into the channel id.** Rejected, because it changes at every fork.
- **Destroying the upgrade key.** Rejected by 0085. Nothing proves a key destroyed, and the
  channels would be stranded at the next fork that raises the transaction version.
- **Waiting for x402 to merge the binding.** Rejected by #1558 and 0085 decision 1. A merge
  reviews the prose and changes nothing about the code holding the money.
- **A custom token first.** Deferred (decision 1).

## Consequences

- **CLAUDE.md's "Mina is gone from this repository" becomes half true.** The old zkApp, the
  faucet's Mina leg and o1js stay gone. A Mina settlement backend will come, in Rust, bound to
  zk_toon's zkApp. CLAUDE.md says so now. Its key table and tier-3 wording change when the
  backend is built.
- **`parse_client_claim` changes shape.** The `mina` refusal moves from "before `scheme`, on
  every carriage" to "at the client edge". A peer carriage reaches the Mina backend.
- **A new crate depends on zk_toon's prover crates.** Their build cost and pinning are the
  backend's to measure.
- **An operator who opts in** declares `mina:<network>/slip44:12586` in `[[tokens]]` and a
  `[[rates]]` row, funds a Mina settlement key with MINA for collateral and fees, waits about an
  hour before a new peering carries traffic, and accepts decision 7's trust statement.
- **`schema_version` moves** when the Mina peer-carriage vectors land.
- **Still open, and owned elsewhere:**
  - the zkApp itself, with its address, VK hash per network, map height and batch size
    (zk_toon#26);
  - recursion in the Rust prover (zk_toon#32);
  - the install runbook and who pays its fee (zk_toon#26);
  - whether TOON builds x402's reference SDK implementation, to decide before #1562's spec PR;
  - what changes for an operator in the guide;
  - whether `toon_cli` and `toon-client` need Mina channels at all.

## Sources

- [`mina-x402-batch-settlement.md`](../research/mina-x402-batch-settlement.md) (#1549): x402 has no
  Mina binding, and every channel transaction needs a proof.
- [`mina-x402-binding-answers.md`](../research/mina-x402-binding-answers.md) (#1552): the voucher,
  ids, tags, amounts, time, delay bounds, roles, and the narrowed `mina` refusal.
- [`rust-zkapp-prover-spike.md`](../research/rust-zkapp-prover-spike.md) (#1561, #1551): proving in Rust.
- [`mina-settlement-confirmation-and-fees.md`](../research/mina-settlement-confirmation-and-fees.md)
  (#1560): landed, counted, waits and fees.
- [`mina-zkapp-upgrades-across-forks.md`](../research/mina-zkapp-upgrades-across-forks.md) (#1559):
  permissions, the upgrade key and the pin.
- [`mina-test-chains.md`](../research/mina-test-chains.md) and [`harness-spawned-test-chains.md`](../research/harness-spawned-test-chains.md)
  (#1554): the lightnet harness.
- The resolution comments on #1550, #1553 and #1558.
- toon-protocol/zk_toon `docs/design/scheme_batch_settlement_mina.md` at `cafb15cf` (zk_toon#34);
  zk_toon#33 and #35 (lightnet confirmations); x402-foundation/x402#4043 (the upstream discussion).
