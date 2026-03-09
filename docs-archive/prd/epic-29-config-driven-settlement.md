# Epic 29: Config-Driven Settlement Infrastructure

**Epic Number:** 29
**Priority:** High — Unblocks multi-node integration testing without `process.env` mutation
**Type:** Brownfield Enhancement — Developer Experience & Testability
**Dependencies:** Epic 28 (in-memory ledger), Epic 6 (settlement foundation), Epic 12 (KeyManager/security)

## Epic Goal

Move settlement keypair and infrastructure configuration from `process.env` into `ConnectorConfig` so that each `ConnectorNode` instance is fully self-contained, enabling multi-node test topologies in a single process without environment variable mutation.

## Epic Description

### Existing System Context

**Current Functionality:**

- `ConnectorNode.start()` reads ~15 settlement-related values from `process.env`: `TREASURY_EVM_PRIVATE_KEY`, `BASE_L2_RPC_URL`, `TOKEN_NETWORK_REGISTRY`, `M2M_TOKEN_ADDRESS`, `PEER{1-5}_EVM_ADDRESS`, `SETTLEMENT_ENABLED`, `SETTLEMENT_THRESHOLD`, `SETTLEMENT_POLLING_INTERVAL`, `INITIAL_DEPOSIT_MULTIPLIER`, `TIGERBEETLE_*`, and `LEDGER_*` env vars
- There is a fragile hack at lines 342-357 of `connector-node.ts` where `process.env.EVM_PRIVATE_KEY` is temporarily swapped to pass the treasury key to `KeyManager`'s `EnvironmentVariableBackend`
- Peer EVM addresses are loaded via a hardcoded `for (let i = 1; i <= 5; i++)` loop reading `PEER{1-5}_EVM_ADDRESS` — limiting the network to 5 peers and requiring env var naming conventions
- Creating two `ConnectorNode` instances with different private keys in the same process requires mutating `process.env` between instantiations — fragile, non-parallelizable, and breaks test isolation

**Technology Stack:**

- TypeScript 5.3.3, Node.js 22 LTS, npm workspaces monorepo
- ethers.js for EVM interactions, `KeyManager` with `EnvironmentVariableBackend`
- `ConnectorNode` factory accepts `ConnectorConfig | string` (object or YAML path)

**Integration Points:**

- `packages/connector/src/config/types.ts` — `ConnectorConfig`, `PeerConfig`, `SettlementConfig` type definitions
- `packages/connector/src/config/config-loader.ts` — Config validation and env var loading
- `packages/connector/src/core/connector-node.ts` — Settlement initialization in `start()`, in-memory ledger creation in `_createInMemoryAccountManager()`
- `packages/connector/src/security/key-manager.ts` + `backends/environment-backend.ts` — Key storage and signing
- `packages/connector/src/settlement/payment-channel-sdk.ts` — Receives KeyManager and provider
- `packages/connector/src/settlement/channel-manager.ts` — Receives private key, peer addresses, token addresses
- `packages/connector/src/settlement/settlement-executor.ts` — Receives private key, peer addresses, token addresses

### Enhancement Details

**What's Being Changed:**

1. **New `SettlementInfraConfig` interface** in `ConnectorConfig` for settlement infrastructure params (private key, RPC URL, registry address, token address, thresholds, etc.) — replaces 15+ env var reads
2. **Extension of `PeerConfig`** with optional per-peer settlement fields (`evmAddress`) — replaces the hardcoded `PEER{1-5}_EVM_ADDRESS` loop and supports arbitrary peer counts
3. **Config-first initialization** in `ConnectorNode.start()` — reads from `this._config.settlementInfra` first, falls back to `process.env` for backward compatibility
4. **Elimination of the `EVM_PRIVATE_KEY` swap hack** by supporting direct private key injection into `KeyManager` (new `DirectKeyBackend` or extended `EnvironmentVariableBackend`)
5. **Multi-node integration test** proving two connectors with distinct keypairs operate independently in a single process

**How It Integrates:**

- The existing `ConnectorConfig` interface gains a new optional `settlementInfra` field. All additions are optional — zero breaking changes.
- `PeerConfig` gains optional `evmAddress` field. Existing configs without it continue to work (env var fallback).
- `ConnectorNode.start()` uses a config-first helper pattern: `this._config.settlementInfra?.privateKey ?? process.env.TREASURY_EVM_PRIVATE_KEY`
- All existing env-var-only deployments (YAML configs, Docker Compose files) continue to work unchanged.

**Success Criteria:**

1. Two `ConnectorNode` instances with different private keys, peer address maps, and RPC URLs can be created and started in the same process without `process.env` mutation
2. All existing tests pass without modification (backward compatibility)
3. YAML config files can optionally specify settlement infrastructure inline
4. No hardcoded `PEER{1-5}` limit — arbitrary peer count supported via `PeerConfig.evmAddress`
5. The `EVM_PRIVATE_KEY` swap hack is eliminated

## Stories

### Story 29.1: Extend ConnectorConfig with Settlement Infrastructure Types

**Description:** Add `SettlementInfraConfig` to `ConnectorConfig` types and extend `PeerConfig` with per-peer settlement fields. Update `ConfigLoader` validation to pass through new fields. Types and validation only — no behavioral changes.

**Acceptance Criteria:**

- New `SettlementInfraConfig` interface in `config/types.ts` with fields: `enabled`, `privateKey`, `rpcUrl`, `registryAddress`, `tokenAddress`, `threshold`, `pollingIntervalMs`, `settlementTimeoutSecs`, `initialDepositMultiplier`, `ledgerSnapshotPath`, `ledgerPersistIntervalMs`
- `PeerConfig` extended with optional `evmAddress?: string` field
- `ConnectorConfig` gains optional `settlementInfra?: SettlementInfraConfig` field
- `ConfigLoader.validateConfig()` passes through new fields without breaking existing validation
- New types exported from `lib.ts`
- TypeScript compilation succeeds, all existing tests pass unchanged

### Story 29.2: Refactor ConnectorNode to Use Config-First Settlement Init

**Description:** Refactor `ConnectorNode.start()` to read settlement configuration from `this._config.settlementInfra` first, falling back to `process.env` for backward compatibility. Eliminate the `EVM_PRIVATE_KEY` swap hack. Build peer address map from `PeerConfig.evmAddress` instead of `PEER{1-5}` env vars.

**Acceptance Criteria:**

- All `process.env.*` settlement reads in `start()` replaced with config-first pattern (config value ?? env var fallback)
- Peer address map built from `this._config.peers` using `peer.evmAddress` field, with `PEER{N}_EVM_ADDRESS` env var fallback for peers without `evmAddress`
- `KeyManager` initialized with direct private key injection — no `process.env.EVM_PRIVATE_KEY` swap hack
- `_createInMemoryAccountManager()` uses config-first for `ledgerSnapshotPath` and `ledgerPersistIntervalMs`
- All existing tests pass without modification (backward compatibility verified)
- Existing Docker Compose deployments work unchanged (env vars still respected)

### Story 29.3: Multi-Node Integration Test with Config-Driven Keypairs

**Description:** Create a multi-node integration test that proves two `ConnectorNode` instances with distinct keypairs can operate independently in a single process using only `ConnectorConfig` — no `process.env` mutation.

**Acceptance Criteria:**

- Test helper function for creating test connector instances with inline settlement config
- Integration test: two connectors (A and B) with distinct Anvil private keys, each configured entirely via `ConnectorConfig.settlementInfra` and `PeerConfig.evmAddress`
- Both connectors start successfully, peer addresses resolved from config, settlement infrastructure initializes independently
- Zero `process.env` mutation during test setup or execution (verified by test assertion or absence of `process.env` writes)
- Test teardown calls `stop()` on both connectors cleanly
- Optional stretch: if Anvil is available, verify payment channel open/deposit between the two nodes

## Compatibility Requirements

- [x] Existing APIs remain unchanged — `ConnectorConfig` additions are all optional fields
- [x] Database schema changes are backward compatible — N/A (no schema changes)
- [x] UI changes follow existing patterns — N/A (no UI changes)
- [x] Performance impact is minimal — config reads replace env var reads (equivalent performance)
- [x] All existing YAML configs work without modification

## Risk Mitigation

- **Primary Risk:** Breaking existing deployments that rely solely on environment variables
- **Mitigation:** Config-first pattern with env var fallback — if `settlementInfra` is not provided in config, behavior is identical to current implementation. All existing env-var-only YAML configs continue to work.
- **Rollback Plan:** Revert the 3 commits (one per story). Stories 29.1 (types only) and 29.2 (behavioral change) can be rolled back independently. Story 29.3 (test only) has zero production impact.

## Definition of Done

- [ ] All stories completed with acceptance criteria met
- [ ] Existing functionality verified through testing (all existing tests pass unchanged)
- [ ] Integration points working correctly (KeyManager, PaymentChannelSDK, ChannelManager, SettlementExecutor)
- [ ] Documentation updated appropriately (inline JSDoc on new types, README if warranted)
- [ ] No regression in existing features
- [ ] Multi-node test passes proving config-driven settlement works
