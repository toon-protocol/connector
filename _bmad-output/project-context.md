---
project_name: 'connector'
user_name: 'Jonathan'
date: '2026-03-09'
sections_completed:
  [
    'technology_stack',
    'language_rules',
    'framework_rules',
    'testing_rules',
    'code_quality',
    'workflow_rules',
    'critical_rules',
  ]
status: 'complete'
rule_count: 67
optimized_for_llm: true
---

# Project Context for AI Agents

_This file contains critical rules and patterns that AI agents must follow when implementing code in this project. Focus on unobvious details that agents might otherwise miss._

---

## Technology Stack & Versions

- **Language:** TypeScript 5.3.3 (strict mode enabled, ES2022 target, CommonJS modules)
- **Runtime:** Node.js >= 22.11.0
- **Monorepo:** npm workspaces (`packages/connector`, `packages/shared`, `packages/contracts`)
- **Blockchain:** ethers 6.16.0 (EVM/Base L2 settlement)
- **Transport:** ws 8.16.0 (BTP over WebSocket, RFC-0023)
- **HTTP:** Express 4.18.x (admin API, health checks, explorer)
- **Logging:** Pino 8.21.0 (structured JSON)
- **Validation:** Zod 3.25.76 (config schemas)
- **Persistence:** better-sqlite3 11.8.1 (claims DB), TigerBeetle 0.16.68 (accounting, optional)
- **Testing:** Jest 29.7.0 + ts-jest 29.1.2
- **Linting:** ESLint 8.56.0 + @typescript-eslint 6.21.0
- **Formatting:** Prettier 3.2.5 (single quotes, trailing commas, 100 char width, LF endings)
- **Git Hooks:** Husky 9.1.7 + lint-staged (pre-commit: eslint --fix + prettier)
- **Releases:** semantic-release 24.2.0 (conventional commits)
- **Contracts:** Solidity (Foundry/Anvil)

## Critical Implementation Rules

### TypeScript Rules

- **Strict mode is fully enabled** — `noUncheckedIndexedAccess`, `noImplicitAny`, `strictNullChecks`, `noUnusedLocals`, `noUnusedParameters`, `noImplicitReturns` are all enforced
- **Array/object index access returns `T | undefined`** — always handle the `undefined` case when accessing by index or key
- **Unused parameters must be prefixed with `_`** — ESLint rule `@typescript-eslint/no-unused-vars` with `argsIgnorePattern: "^_"`
- **No `any` type** — `@typescript-eslint/no-explicit-any: "error"` is enforced
- **Explicit return types encouraged** — `@typescript-eslint/explicit-function-return-type: "warn"` (expressions exempted)
- **No `console.log`** — use Pino logger instead; ESLint `no-console: "error"` (only `console.warn` and `console.error` allowed)
- **Named exports only** — no default exports; separate `export type {}` from runtime exports
- **Use `import type` for type-only imports** — keeps runtime bundles clean
- **Cross-package imports:** use `@toon-protocol/shared` (mapped in Jest via `moduleNameMapper`)
- **Custom Error classes:** set `this.name`, call `Error.captureStackTrace`, use `instanceof` checks
- **Async cleanup:** prefix fire-and-forget async calls with `void` (e.g., `void shutdown('SIGTERM')`)
- **Target ES2022** — can use top-level await, `Array.at()`, `Object.hasOwn()`, etc.

### Framework-Specific Rules

- **Pino logging format:** always `logger.info({ event: 'event_name', key: value }, 'Human-readable message')` — structured fields FIRST, message string SECOND
- **Child loggers:** create via `logger.child({ component: 'component-name' })` for sub-components; inherit parent context (nodeId)
- **Sensitive data:** NEVER log private keys, mnemonics, seeds, or secrets — Pino serializers auto-redact but don't rely on it; actively avoid passing sensitive data to log calls
- **Correlation IDs:** generate via `generateCorrelationId()` → `pkt_{hex}` format; pass as `correlationId` field in log entries for packet tracking across hops
- **Config loading:** YAML config files validated with Zod schemas at startup; use `ConfigLoader` class pattern
- **BTP transport:** class-based with Node.js `EventEmitter` for lifecycle events (connected/disconnected/error); WebSocket-based (ws library)
- **Express usage:** minimal — only for health checks (`GET /health`), admin API, and explorer static serving; NOT the primary transport layer
- **ethers.js:** all blockchain calls are async; use `PaymentChannelSDK` abstraction — never call contract methods directly from business logic
- **Class-based architecture:** major components are classes with constructor-based dependency injection; private fields use `private readonly` pattern
- **EventEmitter pattern:** BTP clients and services extend or compose EventEmitter for lifecycle and state change notifications

### Testing Rules

- **Test files co-located with source:** `module-name.test.ts` next to `module-name.ts` in `src/`; integration tests in `test/integration/`
- **Jest with ts-jest preset:** `testEnvironment: 'node'`, roots `['src', 'test']`, match `**/*.test.ts`
- **Mock logger:** use `pino({ level: 'silent' })` with `jest.spyOn` on methods — NOT plain `jest.fn()` objects; mock `.child()` to return itself
- **Factory functions for test data:** `createMockLogger()`, `createMockAccountManager()`, `createTestPeer()` — keep test setup DRY
- **Type-safe partial mocks:** cast with `as unknown as jest.Mocked<Type>` — never use `any` directly for mock types
- **Private field access in tests:** use `(instance as any)._field` with `// eslint-disable-next-line @typescript-eslint/no-explicit-any`
- **`jest.clearAllMocks()` in `beforeEach`** — always reset mock state between tests
- **Cleanup in `afterEach`:** stop running services/monitors to prevent test leaks
- **Story references:** include story IDs in describe blocks (e.g., `'Feature X (Story 6.4)'`)
- **`jest.mock()` at file top:** mock dependencies before imports are resolved
- **ILP amounts use `BigInt`:** test data uses `100000n` notation, not `Number`
- **Coverage thresholds:** branches 60%, functions 75%, lines 70%, statements 70%
- **Default timeout:** 30s for most tests; specific overrides for integration (60s for security)
- **Cross-package mapping:** `@toon-protocol/shared` mapped to source via `moduleNameMapper` in jest config

### Code Quality & Style Rules

- **File naming:** kebab-case for all files (`settlement-monitor.ts`, `btp-client-manager.ts`)
- **Class naming:** PascalCase (`SettlementMonitor`, `BTPClientManager`)
- **Interface naming:** PascalCase without `I` prefix (`PeerConfig`, not `IPeerConfig`)
- **Private fields:** `private readonly _fieldName` pattern
- **Constants:** `UPPER_SNAKE_CASE` for module-level constants (e.g., `DEFAULT_LOG_LEVEL`)
- **Prettier enforced:** single quotes, trailing commas (es5), 100 char width, 2-space indent, LF endings
- **Source organization by domain:** `btp/`, `core/`, `settlement/`, `routing/`, `config/`, `security/`, `telemetry/`, `utils/`, etc.
- **Public API in `lib.ts`:** all public exports consolidated in `packages/connector/src/lib.ts`; `index.ts` re-exports from `lib.ts`
- **JSDoc on public APIs:** use `@remarks`, `@example`, `@param`, `@returns` tags; include `@packageDocumentation` on module entry points
- **Test file doc comments:** describe test scope and what is being tested at the top of each test file
- **lint-staged pre-commit:** ESLint fix + Prettier on `.ts/.tsx`; Prettier only on `.js/.json/.md`

### Development Workflow Rules

- **Branch naming:** `epic-{number}` for feature branches; `main` is production
- **Commit messages:** Conventional Commits format `{type}({scope}): {description}` — types: `feat`, `fix`, `style`, `qa`, `docs`, `chore`, `security`, `test`; scope is epic number or feature area
- **Pre-commit hook:** lint-staged runs `eslint --fix` + `prettier --write` on staged `.ts/.tsx` files
- **Pre-push hook:** optimized — runs lint/format/related unit tests only for changed source files; auto-skips for docs-only or config-only changes
- **Build order matters:** `packages/shared` MUST build before `packages/connector` (shared provides type definitions); use `npm run build --workspace=packages/shared` first
- **CI gates (required to pass):** lint, format, tests (Node 22.11.0 + 22.x), TypeScript type check, build, EVM contract tests, Aptos Move tests
- **CI gates (advisory):** security audit (npm audit + Snyk), container scan (Trivy), performance benchmark
- **Docker deployment:** images pushed to GHCR on merge to main; multi-platform (amd64 + arm64)
- **Config via YAML:** connector topology defined in YAML config files (see `examples/` for patterns); validated by Zod at startup
- **semantic-release:** version bumps and changelogs auto-generated from conventional commit messages

### Critical Don't-Miss Rules

- **ILP amounts are `BigInt`** — NEVER use `Number` for amounts; values can exceed `Number.MAX_SAFE_INTEGER`; use `100000n` literal notation
- **TigerBeetle is optional** — it's a peer dependency with `optional: true`; code MUST handle its absence gracefully with fallback to in-memory or SQLite
- **BTP has two error types** — `BTPConnectionError` (network) vs `BTPAuthenticationError` (auth); handle both separately in catch blocks
- **ILP packet expiry decrement** — per RFC-0027, connectors MUST reduce packet expiry by safety margin (1s) before forwarding to prevent timeout cascades
- **Settlement is threshold-based** — on-chain settlement triggers on balance or time thresholds, NOT per-packet; per-packet claims are signed and sent via BTP but accumulated off-chain
- **Self-describing claims (Epic 31)** — claims carry chain/contract coordinates in BTP protocolData; receivers verify dynamically on-chain without pre-registration
- **`@toon-protocol/shared` import path** — always `import { Type } from '@toon-protocol/shared'`; never import from dist or relative paths across packages
- **Buffer usage for binary data** — ILP packets use `Buffer` (not `Uint8Array`) for `data`, `executionCondition`, `fulfillment` fields
- **PacketType enum values matter** — `PREPARE=12`, `FULFILL=13`, `REJECT=14` per RFC-0027; don't use arbitrary values
- **Optional dependencies pattern** — many packages are `optionalDependencies`; use dynamic `require()` with try-catch or the project's `optional-require` utility
- **YAML config is the source of truth** — network topology, peers, routes, and settlement config all come from YAML; never hardcode topology
- **ILP addresses are hierarchical** — dot-separated format (e.g., `g.alice.wallet.USD`); validate with `isValidILPAddress()` from shared package

---

## Usage Guidelines

**For AI Agents:**

- Read this file before implementing any code
- Follow ALL rules exactly as documented
- When in doubt, prefer the more restrictive option
- Update this file if new patterns emerge

**For Humans:**

- Keep this file lean and focused on agent needs
- Update when technology stack changes
- Review quarterly for outdated rules
- Remove rules that become obvious over time

Last Updated: 2026-03-09
