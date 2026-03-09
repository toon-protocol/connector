# Epic 28: In-Memory Ledger — Zero-Dependency Accounting

**Epic Number:** 28
**Priority:** High — Eliminates mandatory external service dependency for payment channel functionality
**Type:** Infrastructure — Dependency Reduction & Developer Experience
**Dependencies:** Epic 6 (settlement foundation), Epic 26 (npm publishing readiness), Epic 27 (test optimization — TigerBeetle test cleanup)

## Epic Goal

Replace TigerBeetle as the **default** accounting backend with a zero-dependency, in-memory double-entry ledger that implements the same `TigerBeetleClient` interface, persists snapshots to disk on a configurable interval, and restores state on restart. TigerBeetle remains available as an optional high-performance backend when explicitly configured.

## Epic Description

### Existing System Context

**Current Functionality:**

- `TigerBeetleClient` (6 public methods) wraps `tigerbeetle-node` native addon for account/transfer operations
- `AccountManager` consumes `TigerBeetleClient` for all double-entry bookkeeping: account creation, packet transfer recording, balance queries, credit limit enforcement, settlement threshold detection
- `ConnectorNode` factory checks `TIGERBEETLE_CLUSTER_ID` + `TIGERBEETLE_REPLICAS` env vars — if absent, falls back to a **NoOp stub** that returns zero balances and never triggers settlements
- NoOp stub silently degrades the connector to a stateless packet router — settlements never fire, claims never flow, no warning emitted
- `tigerbeetle-node` is a native addon requiring platform-specific binaries, complicating `npm install` for library consumers (Epic 26 blocker)

**Technology Stack:**

- TypeScript 5.3.3, Node.js 22 LTS, npm workspaces monorepo
- `tigerbeetle-node@0.16.68` as `peerDependency` (optional)
- Account model: debit/credit account pairs per peer/token, 128-bit deterministic IDs via SHA-256

**Integration Points:**

- `packages/connector/src/settlement/tigerbeetle-client.ts` — Client interface to implement
- `packages/connector/src/settlement/account-manager.ts` — Consumer (zero changes needed)
- `packages/connector/src/core/connector-node.ts` — Factory where backend selection happens
- `packages/connector/src/settlement/account-id-generator.ts` — Deterministic ID generation (reused as-is)
- `PacketHandler`, `SettlementMonitor`, `SettlementExecutor`, `AdminServer` — downstream consumers via `AccountManager` (zero changes needed)

### Enhancement Details

**What's Being Changed:**

1. **New `InMemoryLedgerClient`** — A pure TypeScript class (~200-300 lines) implementing the same 6-method interface as `TigerBeetleClient`:
   - `initialize()` / `close()` — Restore snapshot from disk on init, persist + clear timer on close
   - `createAccountsBatch(accounts)` — Idempotent account creation in a `Map<bigint, Account>`
   - `createTransfersBatch(transfers)` — Atomic double-entry transfers (synchronous within Node.js event loop)
   - `getAccountBalance(accountId)` — Direct Map lookup
   - `getAccountsBatch(accountIds)` — Batch Map lookup

2. **Periodic disk persistence** — Configurable interval (default: 30s), atomic write (`write → rename`), dirty flag to skip no-op flushes. Also persists on graceful shutdown (`close()`). Snapshot format: JSON with bigint-as-string serialization.

3. **Factory update in `ConnectorNode`** — When TigerBeetle env vars are absent, instantiate `InMemoryLedgerClient` instead of the NoOp stub. The `AccountManager` receives the in-memory client identically to how it receives the TigerBeetle client — no changes to `AccountManager` or any downstream consumer.

4. **Remove NoOp stub** — The NoOp stub in `connector-node.ts` becomes dead code. Replace it with the in-memory ledger so the connector always has working accounting.

**How It Integrates:**

- `InMemoryLedgerClient` implements the identical interface as `TigerBeetleClient` — it is a **drop-in replacement**
- `AccountManager` receives it via constructor injection — no code changes needed in AccountManager
- All downstream consumers (`PacketHandler`, `SettlementMonitor`, `SettlementExecutor`, `AdminServer`) work unchanged because they only interact through `AccountManager`
- TigerBeetle remains available: if `TIGERBEETLE_CLUSTER_ID` + `TIGERBEETLE_REPLICAS` are set, the connector uses TigerBeetle as before
- Snapshot file path configurable via `LEDGER_SNAPSHOT_PATH` env var (default: `./data/ledger-snapshot.json`)

**Success Criteria:**

1. `npm install @crosstown/connector` works with **zero native addons** required for basic accounting
2. Connector starts with working balance tracking, settlement threshold detection, and claim signing — no TigerBeetle service needed
3. After restart, balances restore from the most recent snapshot within the configured persistence interval
4. All existing `AccountManager` unit and integration tests pass against the in-memory backend
5. TigerBeetle path continues to work unchanged when configured

### Why Not Other Options?

| Option                   | Verdict      | Reason                                                                                                               |
| ------------------------ | ------------ | -------------------------------------------------------------------------------------------------------------------- |
| **Custom in-memory Map** | **Selected** | Zero dependencies, fastest, Node.js atomicity eliminates concurrency concerns, simple persistence via JSON snapshots |
| `better-sqlite3`         | Rejected     | Native addon (same problem as TigerBeetle), adds compilation dependency                                              |
| `sql.js` (SQLite WASM)   | Rejected     | Adds ~2MB WASM dependency, slower than Map, overengineered for this data model                                       |
| LevelDB / RocksDB        | Rejected     | Native addons, designed for disk-first workloads, unnecessary complexity                                             |
| Keep NoOp stub           | Rejected     | Silently broken — settlements never trigger, no accounting, misleading to consumers                                  |

## Stories

### Story 28.1: Implement InMemoryLedgerClient

**Description:** Create `InMemoryLedgerClient` class in `packages/connector/src/settlement/in-memory-ledger-client.ts` that implements the same interface as `TigerBeetleClient`. Uses `Map<bigint, Account>` for account storage with `debits_posted` and `credits_posted` fields. Transfers are synchronous Map mutations (atomic in Node.js single-threaded model). Includes periodic persistence to a JSON snapshot file with atomic write-rename pattern and dirty-flag optimization.

**Acceptance Criteria:**

- Implements all 6 methods: `initialize()`, `close()`, `createAccountsBatch()`, `createTransfersBatch()`, `getAccountBalance()`, `getAccountsBatch()`
- `createAccountsBatch()` is idempotent — creating an existing account is a no-op
- `createTransfersBatch()` validates accounts exist, amount > 0, atomically updates debit + credit accounts
- Persistence: writes snapshot to `{path}.tmp` then renames to `{path}` (atomic replace)
- On `initialize()`: restores from snapshot file if it exists, starts persistence timer
- On `close()`: persists final snapshot, clears timer
- Configurable: `snapshotPath` (string), `persistIntervalMs` (number, default 30000)
- Snapshot format: JSON array of `[accountId_string, { debits_posted_string, credits_posted_string }]` tuples (bigint → string for JSON safety)
- Unit tests covering: account creation, transfers, balance queries, persistence round-trip, idempotency, error cases

### Story 28.2: Integrate InMemoryLedgerClient as Default Backend

**Description:** Update the factory logic in `ConnectorNode` to instantiate `InMemoryLedgerClient` (instead of the NoOp stub) when TigerBeetle env vars are not configured. Remove the `_createNoOpAccountManager()` method. Add `LEDGER_SNAPSHOT_PATH` and `LEDGER_PERSIST_INTERVAL_MS` env var support.

**Acceptance Criteria:**

- When `TIGERBEETLE_CLUSTER_ID` and `TIGERBEETLE_REPLICAS` are absent: `InMemoryLedgerClient` is instantiated and passed to `AccountManager`
- When TigerBeetle env vars are present: existing TigerBeetle path works unchanged
- `_createNoOpAccountManager()` removed from `connector-node.ts`
- Startup log clearly indicates which backend is active: `"Accounting backend: in-memory ledger (snapshot: ./data/ledger-snapshot.json)"` or `"Accounting backend: TigerBeetle (cluster: 0, replicas: localhost:3000)"`
- `LEDGER_SNAPSHOT_PATH` env var configures snapshot location (default: `./data/ledger-snapshot.json`)
- `LEDGER_PERSIST_INTERVAL_MS` env var configures persistence interval (default: `30000`)
- On `ConnectorNode.stop()`: `InMemoryLedgerClient.close()` is called (ensures final snapshot)
- Integration test: start connector without TigerBeetle, send packets, verify balances tracked, restart, verify balances restored

### Story 28.3: Test Suite Compatibility & Documentation

**Description:** Ensure all existing `AccountManager` tests pass against the in-memory backend. Update any test infrastructure that hard-depends on TigerBeetle mocks. Update tech-stack docs and README to reflect the new default backend.

**Acceptance Criteria:**

- All existing `AccountManager` unit tests pass using `InMemoryLedgerClient` as the backend
- Test setup helpers updated to use `InMemoryLedgerClient` by default (no TigerBeetle mock needed)
- `docs/architecture/tech-stack.md` updated: TigerBeetle row notes it is optional, in-memory ledger is the default
- Connector README updated with backend selection documentation
- CI passes without `tigerbeetle-node` installed (validates the zero-dependency story)
- Existing tests that explicitly test TigerBeetle integration remain functional when `tigerbeetle-node` is available

## Compatibility Requirements

- [x] Existing APIs remain unchanged — `AccountManager` public interface is identical
- [x] No schema changes — in-memory ledger uses same account model (debits_posted, credits_posted)
- [x] TigerBeetle path unchanged — env var presence selects backend
- [x] Performance impact is positive — Map operations are faster than network round-trips to TigerBeetle
- [x] npm publish unblocked — no native addon required for default configuration

## Risk Mitigation

- **Primary Risk:** Data loss on crash — up to one persistence interval of balance data lost
- **Mitigation:** Configurable interval (30s default). On-chain payment channel state is the source of truth — the ledger tracks _when_ to trigger settlements, not the actual financial state. Post-restart, settlements may fire slightly early/late until the next threshold check, which self-corrects.
- **Secondary Risk:** Snapshot file corruption on disk-full or power loss
- **Mitigation:** Atomic write-rename pattern — partial writes go to `.tmp` file, only renamed on success. If `.tmp` exists on startup without a valid main file, log a warning and start fresh.
- **Rollback Plan:** Re-set `TIGERBEETLE_CLUSTER_ID` + `TIGERBEETLE_REPLICAS` env vars to revert to TigerBeetle backend. The in-memory ledger code remains in the codebase but is not activated.

## Definition of Done

- [x] All stories completed with acceptance criteria met
- [x] Connector starts and tracks balances without TigerBeetle service running
- [x] Settlements trigger correctly based on in-memory balance thresholds
- [x] Snapshot persistence and restore verified across restarts
- [x] All existing tests pass (no regressions)
- [x] CI pipeline passes without `tigerbeetle-node` installed
- [x] Documentation updated (tech-stack, README)
- [x] TigerBeetle backend still works when explicitly configured
