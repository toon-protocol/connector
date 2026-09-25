# Settlement RPC may ride the circuit, once every wait on it is bounded

**Status:** Proposed. It is waiting on the decision asked for in TOON_Network#167, it binds nothing, and it may be rejected. If it is accepted, it amends [0070](0070-an-onion-address-is-a-host-not-a-carriage.md) **decision 4** and nothing else in that record. It **recommends allowing** proxied settlement RPC, but only together with the backend hardening listed in decision 5. The measurements are in this record. The choice is the maintainer's.

**Scope:** connector architecture, internal to this codebase. One consequence reaches outside it. TOON Network's spec §10 and its ADR 0008 require a Hidden Provider to "run its own settlement RPC". That rule exists because of decision 4, and accepting this record is what would let TOON Network relax it. This record does not change that repository.

**Falsifier:** `crates/**/*.rs` matching `rpc_via_socks_proxy` — this record proposes that key and says the tree does not have it yet; a match means the key was built while the record still reads Proposed, and its Status line is out of date.

**A `[settlement.evm]` or `[settlement.solana]` table may set `rpc_via_socks_proxy = true`.** When it does, every client that dials that table's `rpc_url` goes through the node's one `socks_proxy`, as `socks5h`, on one isolated circuit per chain that stays pinned. That covers the settlement backend, the EVM channel-index syncer and the EVM rate source. There is no direct fallback. This comes **with**, not after, the hardening listed in decision 5: bounded request timeouts, confirmation loops that survive a failed poll, a nonce read from `pending`, and a boot that reads before it transacts. Latency is not what makes a circuit unsafe for settlement. The circuit adds about a quarter of a second to a pooled call and about a second to a fresh one, and that is small next to every deadline settlement has. The danger is that the backends today treat one slow or failed round trip as either **forever** (EVM) or **fatal** (Solana).

## Context

ADR 0070 gave the connector a SOCKS5 path for the ILP wire and deliberately kept settlement off it:

> The proxy covers the ILP wire only. Settlement RPC and `handler_url` dial direct. Routing settlement through a circuit is a separate decision with its own evidence to gather — circuit latency interacts with confirmation semantics and nonce handling on both backends — and is not taken here.

TOON Network builds its Hidden Provider on that fact. A hidden connector that reads a public RPC from its real address links that address to its on-chain identity. So spec §10 and ADR 0008 require a hidden provider to run its own settlement RPC, and the provider's loader refuses one that is not on a loopback or private address (`settlement_rpc_verdict`, provider `src/provider/config.rs`). The #159 deploy bundle's hidden variant therefore needs `HIDDEN_SETTLEMENT_{EVM,SOLANA}_RPC_URL` pointing at nodes the operator runs. By the chains' own published requirements, a Solana RPC node needs hundreds of GB of RAM and several TB of NVMe. In practice no first-time operator will do that, so "hidden" is not available even though it is the headline of the operator pitch.

This record gathers the evidence 0070 asked for:

- what a circuit costs,
- what the backends do with that cost, and
- what a circuit does and does not hide.

## Evidence 1: what a circuit costs

### Method

- **Where it ran.** One workstation on a residential connection. No devnet box was involved.
- **Daemon.** A local `anon` v0.4.10.2 client, the image `local/anon-image` builds, run with `ClientOnly 1`, `SocksPort … IsolateSOCKSAuth` and a `ControlPort`. The anonrc is in the Appendix.
- **Endpoints.** The public devnet endpoints this fleet already uses: `https://api.devnet.solana.com`, `https://sepolia.base.org`, and `https://base-sepolia-rpc.publicnode.com`. That last one is the endpoint this repository's README and the provider bundle's `.env` preset name for Base Sepolia.
- **Calls.** The read shapes the backends issue on their hot paths:
  - Solana: `getLatestBlockhash`, `getSlot`, `getSignatureStatuses`, `getAccountInfo`, all at `confirmed`.
  - EVM: `eth_blockNumber`, `eth_getTransactionCount(…, "pending")`, `eth_getTransactionReceipt`, `eth_gasPrice`.
- **Modes.** Five, round-robined so that every mode met the same network conditions:

| mode               | proxy                                             | connection                                                          |
| ------------------ | ------------------------------------------------- | ------------------------------------------------------------------- |
| `direct-fresh`     | none                                              | new TCP+TLS per call                                                |
| `direct-keepalive` | none                                              | one connection per batch of 8                                       |
| `pinned-fresh`     | `socks5h`, one SOCKS credential for the whole run | new TCP+TLS per call (new stream, same circuit)                     |
| `pinned-keepalive` | as above                                          | one connection per batch: **the shape a pooled reqwest client has** |
| `per-call`         | `socks5h`, a fresh credential per call            | new circuit per call (IsolateSOCKSAuth), new TCP+TLS                |

- **Failures.** A failed call is counted. It is never retried, because the failure rate is part of the result.
- **Circuit build times.** Read from the control port's `CIRC` events, from `LAUNCHED` to `BUILT`.
- **Script.** `tools/bench/settlement-rpc-over-anon.py`, stdlib Python driving `curl`.
- **Samples and timing.** 30 rounds × 8 calls × 5 modes × 2 chains = **2,400 calls**, 2026-09-25 00:17–00:48 UTC. A separate 20-round run against publicnode added **800 calls** (00:23–00:34 UTC).

### Results (total request time, successful calls)

| chain / endpoint                | mode             |   n | fail |    p50 |    p95 |    p99 |     max |
| ------------------------------- | ---------------- | --: | ---: | -----: | -----: | -----: | ------: |
| Solana devnet (api.devnet)      | direct-fresh     | 240 |    0 | 0.166s | 0.206s | 0.401s |  3.163s |
|                                 | direct-keepalive | 240 |    0 | 0.054s | 0.174s | 0.202s |  3.042s |
|                                 | pinned-fresh     | 240 |    0 | 0.947s | 1.541s | 2.813s |  3.343s |
|                                 | pinned-keepalive | 240 |    0 | 0.234s | 1.110s | 1.539s |  3.037s |
|                                 | per-call         | 240 |    0 | 1.612s | 4.281s | 7.482s | 12.988s |
| Base Sepolia (sepolia.base.org) | direct-fresh     | 240 |    0 | 0.143s | 0.178s | 0.190s |  0.202s |
|                                 | direct-keepalive | 240 |    0 | 0.060s | 0.144s | 0.163s |  0.187s |
|                                 | pinned-fresh     | 240 |    0 | 1.052s | 1.821s | 3.862s |  4.936s |
|                                 | pinned-keepalive | 240 |    0 | 0.321s | 1.093s | 1.280s |  1.598s |
|                                 | per-call         | 240 |    0 | 1.702s | 3.591s | 6.110s |  6.896s |
| Base Sepolia (publicnode)       | direct-fresh     | 160 |    0 | 0.154s | 0.203s | 0.258s |  0.336s |
|                                 | direct-keepalive | 160 |    0 | 0.063s | 0.154s | 0.173s |  0.206s |
|                                 | pinned-fresh     | 160 |    0 | 0.965s | 1.479s | 1.969s |  2.315s |
|                                 | pinned-keepalive | 160 |    0 | 0.294s | 1.015s | 1.189s |  1.486s |
|                                 | per-call         | 160 |    0 | 1.889s | 4.607s | 6.646s | 11.520s |

The keepalive rows mix one new connection per batch of 8 with 7 reused calls. Split out (Solana and sepolia.base.org together, n = 420 reused and 60 new per mode):

| keepalive mode | reused connection: p50 / p95 / p99 | new connection: p50 / p99 |
| -------------- | ---------------------------------- | ------------------------- |
| direct         | 0.056s / 0.075s / 0.091s           | 0.158s / 0.303s           |
| pinned circuit | 0.276s / 0.465s / 0.965s           | 1.047s / 1.631s           |

**Circuits.**

- Across both runs (they shared one daemon, and the main run's control-port listener was subscribed throughout), the daemon built 682 circuits and abandoned 238.
- Of the 238, 177 were `TIMEOUT`, which is the daemon's own adaptive build cutoff (computed `TIMEOUT_MS` ≈ 1,024ms at the 80th percentile, `TIMEOUT_RATE` ≈ 0.20). 59 were `DESTROYED` and 2 were `MEASUREMENT_EXPIRED`.
- Abandoned builds never surfaced as a failed call. The daemon retried behind the SOCKS port, and that retrying is already inside the `per-call` latency.
- Build time of the circuits that were used: p50 0.69s, p95 0.98s, max 1.28s. That is bounded by the cutoff, so the `per-call` row, not this figure, is the end-to-end cost of needing a new circuit.

**Bootstrap.**

- Cold start with an empty `DataDirectory`: 13s from start to `Bootstrapped 100%`.
- Restart with its state kept: 3s.

**Failures: none.** 0 of 3,200 calls failed in any mode. That covers exit-side refusals too: no HTTP 403 or 429 from any endpoint, and no curl or SOCKS error. This is one evening's sample from one vantage point, and it is not a guarantee. Public RPCs do rate-limit per IP, and exit IPs are shared. Decision 5 assumes a 429 or 403 will happen eventually and says what then.

### What was not measured

- **Transaction submission and confirmation end to end.** The devnet faucet refused an airdrop (429, daily limit), and this record's author would not spend a fleet key's funds.
- **What a real `sendTransaction` costs.** A `sendTransaction` or `eth_sendRawTransaction` is one JSON-RPC round trip of the same shape as the reads above, and the RPC node relays it onward from **its own** address. So the circuit adds one round trip to submission, and confirmation time is the chain's, observed through polls that each cost one round trip. That is an inference and is labelled as one. A funded run of `submit`/`confirm` through the circuit is the first thing to do if this record is accepted.
- **A circuit dying under a pooled connection mid-request.** Nothing in the sample saw it. By construction it looks like a hung or reset TCP connection, which is why decision 5's timeouts are required rather than nice to have.

### Reading the numbers against settlement's clocks

| clock                                         | value                                                                              | worst circuit figure measured |
| --------------------------------------------- | ---------------------------------------------------------------------------------- | ----------------------------- |
| Solana blockhash lifetime (signing to expiry) | ~150 slots, ~60–90s                                                                | 13s (`per-call` max)          |
| Solana per-request timeout (`HttpSender`)     | 30s                                                                                | 13s                           |
| client-edge liveness TTL / serve-stale window | 60s / 600s (`connector-client-edge/src/channels.rs:274,287`)                       | 13s                           |
| peering challenge period                      | 24h (`PEERING_SETTLEMENT_TIMEOUT_SECONDS`, `connector-runtime/src/peering.rs:104`) | —                             |

A pinned, pooled circuit costs roughly **+0.22s at p50 and +0.9s at p99** per call. A new circuit per call costs **+1.5s at p50 and up to +7s at p99**. Neither comes near any deadline in the table. The cost is real for calls made in sequence: an EVM `redeem` is about 15–20 sequential round trips (five reads, fee and gas estimation, the send, receipt polls, four reads), so on a pinned circuit it goes from about 1–2s to roughly 5–8s. That is slower, and it is still harmless, because nothing on the redeem path is waiting on a clock of seconds.

## Evidence 2: what the backends do under that latency

Read from `origin/main` at `7f523d23`. Dependency behaviour is from the pinned `solana-rpc-client 2.1.0`, `ethers-providers 2.0.14` and `ethers-middleware 2.0.14` sources.

### Solana (`crates/connector-settlement-solana/src/lib.rs`)

- **Client.** `RpcClient::new_with_commitment(rpc_url, confirmed)` (`:192`). This goes through `HttpSender::new`, which sets a 30s whole-request timeout and a 30s pool idle timeout (`http_sender.rs:41-56`). Its built-in retry covers **HTTP 429 only**: up to 5 retries, 500ms apart or at `Retry-After` when that is under 120s. **A 403 is not retried.** There is no websocket.
- **Boot sends a real transaction on every start.** `connect` (`:182`) makes three reads (`getAccountInfo` of the program and of the mint, `getGenesisHash`). It then calls `ensure_own_ata_exists` (`:240`, `:681`), which **submits a create-idempotent ATA transaction every time the node starts**, and then `verify_program_identity` (`getLatestBlockhash` + `simulateTransaction`). Any `Err` fails boot (`connector-cli/src/runtime.rs:749`), and nothing retries.
- **Every write** goes through `submit` (`:711`). It fetches a fresh `getLatestBlockhash`, signs, then calls `send_and_confirm_transaction`. That sends once with preflight at `confirmed` and loops on `getSignatureStatuses`, then `isBlockhashValid`, then sleeps 500ms, until the transaction is confirmed or the blockhash expires (`rpc_client.rs:592-642`).
- **The confirm loop uses `?` on every poll.** One poll that times out, gets a 403, or loses its circuit makes `submit` return `Err` **while the transaction may still land**. This is the one real latency hazard on this backend. The outcome is ambiguous, and it is reported as a failure.
- **Blockhash expiry is a safe failure.** The hash is fetched right before signing, and the whole send-and-confirm has ~60–90s. A stall long enough to expire it (none was measured) fails preflight or ends the loop, and the transaction then cannot land.
- **Re-execution on an operator retry.** Retrying re-signs with a new blockhash. `open` fails because the PDA exists. `redeem` becomes a stale claim because claims are cumulative. `close` and `settle` are refused by their state checks. **`fund` deposits again**, because Deposit is incremental. So after an ambiguous `fund`, a retry over-collateralises the channel. Nothing is lost, but it is locked in the channel.

### EVM (`crates/connector-settlement-evm/src/lib.rs`)

- **No timeouts at all.**
  - `build_client` (`:685`) uses `Provider::<Http>::try_from(rpc_url)` (`:693`), which is `reqwest::Client::new()`: no request timeout, no connect timeout and no retry layer. The same holds for the channel-index syncer (`channel_index_sync.rs:83`) and the rate source (`connector-rate-source-evm/src/lib.rs:136`), which dials the **same** `rpc_url` (`connector-cli/src/runtime.rs:2100-2123`).
  - `PendingTransaction` has no overall deadline.
  - A circuit that stops answering therefore hangs whatever is waiting on it **indefinitely**, whether that is an operator request, `fund` while holding `deposit_lock` (`:573`), a first-seen channel's inline claim lookup, the syncer, or boot.
  - This is a defect **today**, and a direct connection can hit it too. A circuit makes it more likely.
- **Polls every 100ms** (`.interval(100ms)`, `:695`, tuned for Anvil). Over a circuit each poll costs about 0.3s anyway. If an endpoint is returning errors, ethers retries those errors at that pace indefinitely, which is a good way to earn a 429.
- **"Not observed" is declared quickly.**
  - ethers returns `None` after 3 null `eth_getTransactionByHash` polls.
  - `recheck_unobserved` (`:836`) adds 4 receipt reads, 1s apart (`UNOBSERVED_RECHECK_ATTEMPTS`, `:821`).
  - The error text is honest: it says "not proof it was dropped". But there is no reconciliation after it. At circuit latency that window is about 5–10s, against a transaction that usually does land.
- **The nonce manager's recovery path is unsafe on an ambiguous send** (`ethers-middleware nonce_manager.rs:68-80, 138-165`).
  - The local nonce is seeded from the `latest` count, not `pending`.
  - On **any** send error (including a timeout after the node already accepted the transaction), it re-reads `latest`, stores it and resubmits at that nonce. It does not advance the local nonce afterwards, so the next transaction reuses that nonce and fails once.
  - With two operations in flight (only `fund` is serialised), rewinding to `latest` can target a nonce that a **different** pending transaction holds, and with a fee rise of 10% or more it replaces that transaction.
  - A circuit makes "send error after acceptance" much more common, and that turns this path from theoretical into likely.
- **Boot** (`connect`, `:129`) makes three reads (`eth_chainId`, `getTokenNetwork`, `decimals`). It sends no transaction, and with no timeout, a stalled circuit hangs boot instead of failing it.

### Where a slow RPC could lose money

In one place only, and it is not latency-specific. Suppose a counterparty closes a channel and this node does not `redeem` its latest claim before `closed_at + settlement_timeout`. The counterparty can then settle without that claim. Redeem is operator-triggered, and there is no watcher. With the peering default of 24h, the RPC would have to be unusable for most of a day. With an operator-chosen window of an hour it would be more exposed, but still by many orders of magnitude more than a circuit's seconds. Everything else a slow RPC causes is a **delay**, a **refused or late claim** (the client's packet expires while a first-seen channel is read), or an **ambiguous outcome that a human has to reconcile**.

## Evidence 3: what a circuit hides, and what it does not

Payments are public on chain either way. Every channel, deposit, claim redeemed and settlement names this node's addresses. Every counterparty knows those addresses too. A circuit changes **who learns the node's network address alongside that identity**. It does not change the identity or the payments.

| observer                       | direct to a public RPC                                                                                                                                              | through a pinned circuit                                                                                                                                  | self-hosted RPC node                                                                                                                                                                                |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| RPC provider                   | **the node's IP**, joined to every address it reads (`getAccountInfo` of its channels and ATA, `eth_getTransactionCount` of its key) and every transaction it signs | an exit relay's IP. It still sees every query and transaction, all of which name this node's keys, so it can **profile the operator but not locate them** | nothing (there is no third-party RPC)                                                                                                                                                               |
| the operator's ISP / LAN       | connections to the RPC's hostname (SNI) and timing                                                                                                                  | connections to an Anyone guard; not the destination                                                                                                       | a chain node's p2p traffic, which is recognisably a chain node                                                                                                                                      |
| the chain's p2p network        | nothing (the RPC relays the transaction from its own address)                                                                                                       | nothing (same)                                                                                                                                            | **the node's IP in the cluster's gossip table** (every Solana node advertises its contact info), and it is the first hop of every transaction it submits unless its own p2p traffic is also proxied |
| a guard / exit relay           | —                                                                                                                                                                   | the guard sees the node's IP but not the destination. The exit sees the RPC's hostname and timing, but not the content (TLS)                              | —                                                                                                                                                                                                   |
| a global or colluding observer | trivially everything                                                                                                                                                | end-to-end timing correlation of guard and RPC traffic remains possible. Onion routing does not defend against a global passive adversary                 | the chain node's own traffic is an identifier                                                                                                                                                       |

**A circuit per call buys nothing against the RPC provider.** Every call names the same keys, so the provider links them by content however many circuits carry them. The only other observers who could link calls are exit relays, and a pinned circuit gives one exit about 10 minutes of a node's settlement traffic (`MaxCircuitDirtiness`, after which new connections move to a new circuit). That traffic is hostnames and timing only. Per-call isolation costs 1.5s at p50 and up to 13s at the tail, and it makes the node's traffic look distinctive to its guard. Hence decision 3, which pins **per chain**, isolated from the ILP wire's streams. That way no single exit sees both chains' RPCs, and the peering traffic does not share a circuit with them.

**What it does not hide, stated plainly:**

- who paid whom, how much, and when (on chain);
- that the node's settlement traffic comes from the Anyone network (exit lists are public, and an RPC provider can see that its client is an exit);
- anything about the operator given away by an **API-keyed** RPC. A keyed endpoint links every query to the account that holds the key, and its email and payment method. A hidden operator must use a keyless endpoint, or accept that link.

**Self-hosting is stronger for reads, and not automatically for writes.** A self-hosted node hides reads completely. It also puts the operator's IP into the chain's peer-to-peer layer: Solana gossip publishes it, and a transaction's first p2p hop is a well-known origin signal. So "run your own node" is the stronger option only if that node's own p2p egress is proxied or otherwise unlinkable, which spec §10 does not say today. This record does not decide that. It flags it for the maintainer, because the recommendation "keep self-hosting as the stronger option" in TOON_Network#167 step 3 depends on it.

## Decision (proposed)

1. **`rpc_via_socks_proxy` per settlement table.**
   - `[settlement.evm]` and `[settlement.solana]` each accept `rpc_via_socks_proxy`, a boolean that defaults to `false`.
   - A table that sets it to `true` with no root `socks_proxy` refuses to load.
   - The key selects the node's one proxy. It does not name a second one. ADR 0070's "a node configures one SOCKS5 proxy" stands.
   - This is an explicit opt-in and not host selection, because a public RPC's host carries no signal the way a `.onion` or `.anyone` host does. For the same reason, 0070's rejection of "proxy every outbound dial once a proxy is configured" stands.

2. **It covers every client of that `rpc_url`, and fails closed.**
   - For EVM that means three clients: the settlement backend, `EvmChannelIndexSyncer`, and the EVM rate source (which dials the EVM table's `rpc_url` today). For Solana it means the settlement backend.
   - One of these left direct is the whole leak, so the implementation builds the proxied client once, per table, and hands it to all of them.
   - The URL must be `socks5h`, so no hostname is resolved locally.
   - A proxy that is down is an error. It never falls back to a direct dial.
   - `handler_url` stays direct. It is loopback in every provider deployment, and nothing here concerns it.

3. **One pinned circuit per chain, isolated from the peer wire.**
   - Each settlement table's client authenticates to the SOCKS port with its own fixed username (for example `toon-settlement-evm` and `toon-settlement-solana`), so `IsolateSOCKSAuth` keeps each chain on its own circuit and off the ILP wire's circuits.
   - The daemon's own `MaxCircuitDirtiness` rotation is left in place.
   - Circuit-per-call is rejected (Evidence 3).

4. **Timeouts, sized from Evidence 1 with a margin of at least twice the worst value seen.**
   - A **connect** timeout of 20s covers the SOCKS handshake, circuit, TCP and TLS. The worst new-circuit call measured was 13s.
   - A **request** timeout of 30s: Solana's existing figure, now applied to EVM too.
   - A pool idle timeout of 30s or less, so a connection whose circuit died quietly is not reused for long.
   - These apply to **all** settlement clients, proxied or not. The EVM backend's unbounded requests are a defect on a direct connection as well.

5. **The hardening this record is conditional on.** Without these, it recommends **against** allowing the key.
   - **Solana confirm tolerates a failed poll.** Replace `send_and_confirm_transaction` with `send_transaction` plus a loop that keeps polling after a transient error, with backoff, until `getBlockHeight` passes the `lastValidBlockHeight` returned with the blockhash. It then reports the outcome by signature: confirmed, failed, or **expired, so it cannot land**. It never reports a bare error for a transaction that may have landed.
   - **EVM confirm has a deadline and reconciles.**
     - Wrap `confirm()` in an overall deadline of 180s (90 Base blocks).
     - Raise the poll interval to 1s when proxied. It is 100ms today and tuned for Anvil.
     - Widen `recheck_unobserved` to cover at least 30s.
     - On an ambiguous send or confirm, look the transaction up **by hash** before anything resubmits.
   - **EVM nonces come from `pending`, and nothing resubmits blindly.** Replace `NonceManagerMiddleware`'s error path with a serialised sender that seeds from `eth_getTransactionCount(…, "pending")`. After a send error, it checks by hash and never re-sends at a rewound nonce.
   - **Boot reads before it transacts, and retries.** `ensure_own_ata_exists` first reads the ATA with `getAccountInfo` and submits only when the ATA is missing, which is once in a key's life rather than on every start. Boot's RPC calls retry a bounded number of times (three, with backoff) before failing the node.
   - **403 and 429 are retried with backoff on both chains,** within the request budget, and are never treated as a final answer on a confirmation poll. None was seen in 3,200 calls, but exit IPs are shared, and the Solana client today retries only a 429.
   - **`fund` is idempotent under retry.** It is the one operation that can execute twice after an ambiguous outcome on both chains. It needs either a stated target total checked before it sends, or a documented rule that it is never retried without reading the channel first.

## Considered options

- **Reject, and keep "a hidden provider runs its own settlement RPC".** This is the status quo, and it stays available as the stronger option for reads. It is rejected as the _only_ option because it makes "hidden" unavailable to every realistic operator (TOON_Network#167), and because self-hosting leaks the IP at the p2p layer instead (Evidence 3).
- **Allow it with a new circuit per call.** Rejected in Evidence 3: it costs about seven times the latency (1.6s against 0.23s at p50) and buys nothing against the RPC, which links the calls by their content.
- **Proxy settlement whenever `socks_proxy` is set.** This is 0070's rejected "proxy everything" again. A node with one onion peer and a self-hosted, private-network RPC would have that RPC routed out through an exit for no reason. Settlement is opted in per table, where the operator knows what `rpc_url` is.
- **A per-table `socks_proxy = "socks5h://…"` URL instead of a boolean.** Rejected. It lets two proxies exist on one node, and two places saying one thing is how a dial ends up going the wrong way (0070's argument against a per-peer `proxy =`).
- **Allow the key now and harden later.** Rejected. The EVM path's unbounded waits and the nonce manager's rewind are exactly the failures a circuit makes common. Shipping the key first would take today's rare defects and make them routine for the operators who are least able to reconcile a stuck nonce by hand.
- **Use an API-keyed RPC provider behind the circuit.** Not rejected, but not recommended. It is the operator's choice, and it gives the provider an account-level link to the operator's identity. Evidence 3 says so, and the operator documentation would have to say so too.

## Consequences

- **A hidden provider stops needing a chain node.** Once the key and the hardening land, the #159 bundle's hidden variant can point `[settlement.*]` at public endpoints with `rpc_via_socks_proxy = true`. That depends on TOON Network changing spec §10 and ADR 0008, and the provider's loader, which this record does not do.
- **Settlement gets slower and stays correct.** Pooled calls cost about 0.2–1s more each, so an EVM redeem goes from seconds to several seconds. Nothing on those paths has a deadline measured in seconds.
- **The hardening helps every node, not just hidden ones.** Bounded requests, confirm loops that survive a failed poll, `pending` nonces and a read-first boot fix defects a direct connection can hit today.
- **The linkage 0070 left standing closes for the RPC provider and for the node's ISP, and nothing else changes.** The operator's IP stops being joined to its on-chain identity at the RPC. The payments, the addresses and the counterparties are exactly as public as before.
- **A new dependency on the anonymity network's health.** If `anon` is down, settlement is down. It fails closed and loudly, never direct. This is the same bargain the hidden ILP wire already makes. In this repository's CI it means the same thing 0070 said: a SOCKS dial test against a local SOCKS5 server, never a real-`anon` gate.

## What would make this record true, or false

- **True:** the maintainer accepts it on TOON_Network#167. Decision 5 lands. A funded run of `submit`/`confirm` over a pinned circuit on both devnets shows confirmation within the budgets in decision 4 (the gap named under "What was not measured").
- **False:** the maintainer rejects it. Or a longer or differently placed sample shows sustained exit-side refusal (403 or 429) from the public devnet RPCs at a rate the retry budget cannot absorb. That is the one outcome of Evidence 1 that would have changed this recommendation, and a single evening cannot rule it out.

## Appendix: reproducing the measurement

`anonrc` (mounted read-only at `/etc/anon/anonrc`; the password hash comes from `docker run --rm --entrypoint anon anon-live:v0.4.10.2 --AgreeToTerms 1 --hash-password <pw>`):

```
AgreeToTerms 1
User anond
DataDirectory /var/lib/anon
Nickname toon167measure
ClientOnly 1
ORPort 0
DirPort 0
ControlSocket 0
SocksPort 0.0.0.0:9050 IsolateSOCKSAuth
SocksPolicy accept 10.0.0.0/8
SocksPolicy accept 172.16.0.0/12
SocksPolicy accept 192.168.0.0/16
SocksPolicy accept 127.0.0.0/8
SocksPolicy reject *
ControlPort 0.0.0.0:9051
HashedControlPassword 16:<hash>
Log notice stdout
```

```bash
docker build -t anon-live:v0.4.10.2 local/anon-image     # if not already built
docker run -d --name anon167 --init -v "$PWD/anonrc:/etc/anon/anonrc:ro" \
  -p 127.0.0.1:19050:9050 -p 127.0.0.1:19051:9051 anon-live:v0.4.10.2

tools/bench/settlement-rpc-over-anon.py --password <pw> --rounds 30 --batch 8 --out target/bench-167
tools/bench/settlement-rpc-over-anon.py --password <pw> --only base-sepolia \
  --evm-url https://base-sepolia-rpc.publicnode.com --rounds 20 --batch 8 --out target/bench-167-publicnode
```

Each call is a line in `<out>/calls.jsonl` (curl's `time_total`, `time_connect`, `time_appconnect`, `num_connects`, HTTP code, exit code and the first 300 bytes of the body), and each circuit event is a line in `<out>/circuits.jsonl`. `--summarize <out>` reprints the table. The two runs above shared one daemon, so their circuit logs overlap. The figures under "Circuits" are from the main run's log, which covers both.
